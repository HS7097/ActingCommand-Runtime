// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #453 S2 (register §1.362; contracts/scheduling-pause.md, "Pauses held at a first
// start"; contracts/runtime-fact-store.md, `host.start_pause_applied`): an entry that declares
// `start_paused` is held by the first start that sees it, with the instance pause `instance.added`,
// before any instance is prepared, and recorded in the tagged runtime fact
// `host.start_pause_applied` after its pause, so no later start holds it again. Fix 1 (coordinator
// work order `rd-1114/brief-1704-fix1.md`): the member is applied only by its own pause (an entry
// first seen under a pause of another reason is held with `instance.added` when that pause is
// released), each record of the fact keeps only the configured instances' rows, and a
// configuration without the member is compared with a host whose start and release never enter
// the slice's code path. Host starts on temp state roots with the fixture the start-hold tests use
// (`config`, `FakeProvider`; the S6c and PAUSE-AUTO helpers for the configured emulator start at
// daemon start; W2-B's startup package queued behind a pause). Two instances, FIRST declaring the
// member and SECOND not, unless a test says otherwise. Every wait returns as soon as what it waits
// for is reported or recorded; `GATE_WAIT` only bounds a broken run. No test reads a clock: times
// are compared only as values read back from the same root.

use super::emulator_controller::EmulatorCall;
use super::gate_369_s6c::{
    GATE_WAIT, START, autostart_host, claims_after, connectivity, control, head, instance_scope,
    pause, resume, send, stop,
};
use super::*;
use crate::codes::HostCode;
use crate::host::{START_PAUSE_APPLIED_REFUSED_FOR_TEST, START_PAUSE_OFF_FOR_TEST};
use actingcommand_contract::codes::SanitizationCode;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    ConfigParameter, ConfigParameterSource, FactScalar, INSTANCE_ADDED_PAUSE_REASON,
    InstancePauseStage, InstancePauseState, OwnerEpoch, RuntimeConfigManifest, RuntimeFactScope,
    RuntimePayload, RuntimeSubscriptionRequest, SchedulingPauseExpectation, SchedulingPauseReason,
    SubscriptionCursor,
};
use actingcommand_host_metrics::NetworkConnectivity;
use actingcommand_runtime_config::scheduling::DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS;
use actingcommand_scheduler::ClaimKind;
use serde_json::{Value, json};

const FIRST: &str = "start_paused.first";
const SECOND: &str = "start_paused.second";
/// FIRST's instance id under another alias (test 8).
const RENAMED: &str = "start_paused.renamed";
/// The entry that stays configured over many starts (test 6).
const KEEP: &str = "start_paused.keep";
/// The fact of the entries whose first-start pause was applied.
const APPLIED: &str = "host.start_pause_applied";
/// The fact of the held pauses.
const PAUSES: &str = "host.scheduling_pause";
/// The reason of an operator's pause.
const OPERATOR_PAUSE: &str = "s2_operator_pause";
/// The reason of the installer's hold (`--hold <reason>`).
const INSTALLER_HOLD: &str = "s2_install_hold";

/// A start on `root` of FIRST (`ids.0`) and SECOND (`ids.1`), each on a default fake device, with
/// the instances of `declared` declaring `start_paused`.
fn start(
    root: &TempDir,
    ids: (InstanceId, InstanceId),
    declared: &[InstanceId],
) -> RuntimeHostResult<RuntimeHost> {
    start_entries(
        root,
        &[(FIRST.to_owned(), ids.0), (SECOND.to_owned(), ids.1)],
        &declared.iter().copied().collect(),
    )
}

/// A start on `root` of `entries` (alias and instance id), each on a default fake device, with
/// the instances of `declared` declaring `start_paused`.
fn start_entries(
    root: &TempDir,
    entries: &[(String, InstanceId)],
    declared: &BTreeSet<InstanceId>,
) -> RuntimeHostResult<RuntimeHost> {
    RuntimeHost::start(
        config(root).with_start_paused(declared.clone()),
        Arc::new(FakeProvider::from_entries(
            entries
                .iter()
                .map(|(alias, id)| (alias.clone(), *id, Arc::new(FakeState::default())))
                .collect::<Vec<_>>(),
        )),
    )
}

/// `id` as the configuration and `host.start_pause_applied` spell it.
fn id_text(id: InstanceId) -> String {
    match serde_json::to_value(id).expect("an instance id encodes") {
        Value::String(text) => text,
        other => panic!("an instance id is text: {other}"),
    }
}

/// The instance pauses the host holds, by alias; the global pause holds nothing here.
fn instance_pauses(host: &RuntimeHost) -> BTreeMap<String, InstancePauseState> {
    let (global, instances) = host.scheduling_pauses_for_test().expect("the held pauses");
    assert!(global.is_none(), "no global pause: {global:?}");
    instances
}

/// The aliases the host holds paused, in order.
fn paused_aliases(host: &RuntimeHost) -> Vec<String> {
    instance_pauses(host).into_keys().collect()
}

/// The rows of `host.start_pause_applied` the host holds, if it holds the fact.
fn applied_rows(host: &RuntimeHost) -> Option<Vec<BTreeMap<String, FactScalar>>> {
    host.runtime_fact_for_test(&RuntimeFactScope::Runtime, APPLIED)
        .expect("the program facts")
        .map(|record| match record.value {
            ContractFactValue::RecordList(rows) => rows,
            other => panic!("host.start_pause_applied is a record list: {other:?}"),
        })
}

/// The instance ids the applied rows name, in row order; each row is exactly its `instance_id`
/// and its `applied_at_unix_ms`.
fn applied_ids(host: &RuntimeHost) -> Vec<String> {
    applied_rows(host)
        .unwrap_or_default()
        .iter()
        .map(|row| {
            assert_eq!(row.len(), 2, "{row:?}");
            assert!(
                matches!(
                    row.get("applied_at_unix_ms"),
                    Some(FactScalar::TimestampMs(_))
                ),
                "{row:?}"
            );
            match row.get("instance_id") {
                Some(FactScalar::String(id)) => id.clone(),
                other => panic!("a row names its instance id: {other:?}"),
            }
        })
        .collect()
}

/// Each `runtime.fact_recorded` in the root's ledger, in ledger order: its sequence, whether its
/// scope is the Runtime's, and its key.
fn fact_records(host: &RuntimeHost) -> Vec<(u64, bool, String)> {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(EventType::RuntimeFactRecorded),
        ..EventQuery::default()
    })
    .expect("the fact records")
    .iter()
    .filter_map(|event| match event.payload() {
        EventPayload::Runtime(RuntimePayload::FactRecorded(recorded)) => Some((
            event.sequence(),
            recorded.record().scope == RuntimeFactScope::Runtime,
            recorded.record().key.clone(),
        )),
        _ => None,
    })
    .collect()
}

/// The sequences of the records of `key`, in ledger order.
fn recorded(host: &RuntimeHost, key: &str) -> Vec<u64> {
    fact_records(host)
        .into_iter()
        .filter(|(_, _, recorded)| recorded == key)
        .map(|(sequence, _, _)| sequence)
        .collect()
}

/// The sequences of the records of `key` after `after`, in ledger order.
fn recorded_after(host: &RuntimeHost, key: &str, after: u64) -> Vec<u64> {
    recorded(host, key)
        .into_iter()
        .filter(|sequence| *sequence > after)
        .collect()
}

fn reported(lines: &[Outcome], code: &str) -> usize {
    lines
        .iter()
        .filter(|line| line.code().as_str() == code)
        .count()
}

/// The daemon start's report lines once `done` holds for them (none is taken).
fn await_reports(host: &RuntimeHost, done: impl Fn(&[Outcome]) -> bool) -> Vec<Outcome> {
    host.emulator_autostart_reports()
        .await_lines_for_test(done, GATE_WAIT)
        .expect("the awaited report lines")
}

/// The Starts the scripted emulator was asked for.
fn control_starts(state: &FakeState) -> usize {
    state
        .emulator
        .calls()
        .iter()
        .filter(|call| **call == EmulatorCall::Control(START))
        .count()
}

/// FIRST's pause as the hold sets it: `instance.added`, revision 1 (the scope's first), stage
/// `released`.
fn assert_held_added(pause: &InstancePauseState) {
    assert_eq!(pause.pause_reason.as_str(), INSTANCE_ADDED_PAUSE_REASON);
    assert_eq!(pause.revision, 1);
    assert_eq!(pause.stage, InstancePauseStage::Released);
}

/// The records of `event_type` in the root's ledger.
fn records_of(host: &RuntimeHost, event_type: EventType) -> usize {
    host.query_persisted_events_for_test(EventQuery {
        event_type: Some(event_type),
        ..EventQuery::default()
    })
    .expect("the records of the type")
    .len()
}

/// Returns once a record of `event_type` after `after` is recorded (a ledger subscription that
/// answers as soon as it is; `GATE_WAIT` only bounds a broken run).
fn await_recorded(host: &RuntimeHost, event_type: EventType, after: u64) {
    let subscription = RuntimeSubscriptionRequest::new(
        EventQuery {
            event_type: Some(event_type),
            from_sequence: Some(after + 1),
            ..EventQuery::default()
        },
        ProjectionProfile::Forensic,
        SubscriptionCursor::default(),
        u64::try_from(GATE_WAIT.as_millis()).expect("gate wait"),
        1,
    )
    .expect("subscription request");
    let receipt = send(
        host,
        RuntimeOperation::SubscribeEvents {
            request: subscription,
        },
        4599,
    );
    let Some(RuntimeResult::EventBatch { batch }) = receipt.result() else {
        panic!("an event batch: {receipt:?}");
    };
    assert!(!batch.timed_out(), "{event_type:?} after {after}");
}

/// The owner epoch the host's status names.
fn owner_epoch(host: &RuntimeHost) -> OwnerEpoch {
    let receipt = send(host, RuntimeOperation::Status, 4598);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("a status: {receipt:?}");
    };
    status.owner_epoch()
}

/// Who releases FIRST's pause.
#[derive(Clone, Copy, Debug)]
enum Release {
    /// A person's resume (`ResumeScheduling` without a condition: the console, `actingctl
    /// resume`, the Lab's `resume`).
    Person,
    /// The installer's release of its hold: `ResumeScheduling` with the owner epoch and the
    /// revision the Runtime shows (`release-hold`, `resume_seen`).
    Installer,
}

impl Release {
    /// The reason of the pause this party releases.
    const fn reason(self) -> &'static str {
        match self {
            Self::Person => OPERATOR_PAUSE,
            Self::Installer => INSTALLER_HOLD,
        }
    }
}

/// FIRST's instance pause with `reason`, as a person's or the installer's `pause` sends it.
fn pause_first(host: &RuntimeHost, reason: &str, connection: u64) -> RuntimeReceipt {
    send(
        host,
        RuntimeOperation::PauseScheduling {
            scope: instance_scope(FIRST),
            pause_reason: SchedulingPauseReason::new(reason).expect("pause reason"),
            drain_timeout_ms: 5_000,
        },
        connection,
    )
}

/// The release of FIRST's pause by `release`.
fn release_first(host: &RuntimeHost, release: Release, connection: u64) -> RuntimeReceipt {
    let expected = match release {
        Release::Person => None,
        Release::Installer => Some(SchedulingPauseExpectation {
            owner_epoch: owner_epoch(host),
            revision: instance_pauses(host)[FIRST].revision,
        }),
    };
    send(
        host,
        RuntimeOperation::ResumeScheduling {
            scope: instance_scope(FIRST),
            expected,
        },
        connection,
    )
}

/// A start on `root` of FIRST alone (`first`), discovery-bound to its scripted emulator `state`
/// with opens that pass a preparation's self-check, with the neutral startup package; FIRST
/// declares `start_paused` when `declared`. A start on a root an earlier start left finds the
/// emulator bound as that start left it.
fn startup_start(
    root: &TempDir,
    first: InstanceId,
    state: &Arc<FakeState>,
    declared: bool,
) -> RuntimeHost {
    state.physical_task_geometry.store(true, Ordering::Release);
    state
        .transition_capture_after_input
        .store(true, Ordering::Release);
    state.observed_open.store(true, Ordering::Release);
    let package = root.path().join("startup-package.zip");
    fs::write(&package, neutral_contained_task_package(true)).expect("write the startup package");
    let startup = ContainedTaskRequest::new(package.to_string_lossy().into_owned())
        .and_then(|request| request.with_response_deadline_ms(60_000))
        .expect("the startup package request");
    let declared = if declared {
        BTreeSet::from([first])
    } else {
        BTreeSet::new()
    };
    let host = RuntimeHost::start(
        config(root)
            .with_startup_packages(BTreeMap::from([(FIRST.to_owned(), startup)]))
            .with_start_paused(declared),
        Arc::new(FakeProvider::one(FIRST, first, Arc::clone(state))),
    )
    .expect("a start with the startup package");
    if !state.emulator_control.load(Ordering::Acquire) {
        bind_fake_emulator(&host, FIRST, first, state);
    }
    host
}

// (1) A first start of an entry with the member: FIRST is paused with instance.added, revision 1,
// stage released, and SECOND is not; the applied row names FIRST's id; the pause row is recorded
// before the applied row, one record each. The daemon start's configured emulator start (both
// instances name it, both emulators stopped) starts SECOND's emulator and not FIRST's: no claim on
// FIRST, no Start. Fix 1 (review item 4): the hold comes before any instance is prepared. On a
// fixture whose instances the start prepares (each with its own ADB endpoint), the start opens
// nothing on FIRST's device, which a hold after the preparation would have opened, and opens
// SECOND's, so the preparation ran.
#[test]
fn s2_1_a_first_start_holds_the_entry_records_it_after_its_pause_and_starts_no_emulator_of_it() {
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    let (first, second) = (instance_id(), instance_id());
    let (read, _reads) = connectivity(Vec::new(), NetworkConnectivity::Internet);
    let host = autostart_host(
        &root,
        &[
            (FIRST, first, &first_state),
            (SECOND, second, &second_state),
        ],
        read,
        |config| config.with_start_paused(BTreeSet::from([first])),
    );

    let pauses = instance_pauses(&host);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    assert_held_added(&pauses[FIRST]);
    assert_eq!(applied_ids(&host), [id_text(first)]);
    let pause_records = recorded(&host, PAUSES);
    let applied_records = recorded(&host, APPLIED);
    let ([pause_record], [applied_record]) = (&pause_records[..], &applied_records[..]) else {
        panic!("one pause record and one applied record: {pause_records:?} {applied_records:?}");
    };
    assert!(
        pause_record < applied_record,
        "the pause row first, the applied row second"
    );

    stop(&host, FIRST, 4531);
    stop(&host, SECOND, 4532);
    let before = head(&host);
    assert_eq!(
        host.enqueue_emulator_autostart().expect("enqueue"),
        1,
        "SECOND alone"
    );
    let lines = await_reports(&host, |lines| {
        reported(lines, "emulator_autostart_started") == 1
    });
    let started = lines
        .iter()
        .find(|line| line.code().as_str() == "emulator_autostart_started")
        .expect("the start");
    assert_eq!(started.value("alias"), Some(&json!(SECOND)));
    assert_eq!(control_starts(&second_state), 1, "SECOND's emulator starts");
    assert_eq!(claims_after(&host, second, before).len(), 1);
    assert_eq!(control_starts(&first_state), 0, "FIRST's does not");
    assert!(
        claims_after(&host, first, before).is_empty(),
        "no claim on FIRST"
    );
    assert_eq!(
        paused_aliases(&host),
        [FIRST.to_owned()],
        "FIRST stays paused"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close host");

    // The hold before any preparation, on a fixture whose instances the start prepares.
    let root = TempDir::new().expect("tempdir");
    let first_state = Arc::new(FakeState::default());
    let second_state = Arc::new(FakeState::default());
    for state in [&first_state, &second_state] {
        state.physical_task_geometry.store(true, Ordering::Release);
        state
            .transition_capture_after_input
            .store(true, Ordering::Release);
        state.observed_open.store(true, Ordering::Release);
    }
    let host = RuntimeHost::start(
        config(&root).with_start_paused(BTreeSet::from([first])),
        Arc::new(
            FakeProvider::from_entries(vec![
                (FIRST.to_owned(), first, Arc::clone(&first_state)),
                (SECOND.to_owned(), second, Arc::clone(&second_state)),
            ])
            .with_device_endpoints(),
        ),
    )
    .expect("a start that prepares its instances");
    assert_eq!(paused_aliases(&host), [FIRST.to_owned()]);
    assert_held_added(&instance_pauses(&host)[FIRST]);
    assert_eq!(
        (
            first_state.open_count.load(Ordering::Acquire),
            first_state.capture_open_count.load(Ordering::Acquire),
        ),
        (0, 0),
        "nothing was opened on the held instance's device before or after the hold"
    );
    assert!(
        second_state.open_count.load(Ordering::Acquire) > 0,
        "the daemon-start preparation opened SECOND's device"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the preparing host");
}

// (2) After a person resumed FIRST, a restart holds nothing: no pause, and the applied row is the
// one the first start recorded (read back from the same root), with no second record.
#[test]
fn s2_2_after_the_resume_a_restart_holds_nothing_and_leaves_the_applied_row() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    let host = start(&root, ids, &[ids.0]).expect("the first start");
    assert_eq!(paused_aliases(&host), [FIRST.to_owned()]);
    let rows = applied_rows(&host).expect("the applied row");
    let receipt = resume(&host, instance_scope(FIRST), 4533);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert!(paused_aliases(&host).is_empty(), "resumed");
    host.close().expect("close the first host");

    let restarted = start(&root, ids, &[ids.0]).expect("the restart");
    assert!(paused_aliases(&restarted).is_empty(), "not paused again");
    assert_eq!(
        applied_rows(&restarted),
        Some(rows),
        "the applied row unchanged"
    );
    assert_eq!(recorded(&restarted, APPLIED).len(), 1, "no second record");
    assert!(restarted.fatal_error().expect("runtime health").is_none());
    restarted.close().expect("close the restarted host");
}

// (3) Fix 1: FIRST paused for another reason (an operator's pause, restored by the start) when the
// member is first seen: that pause stays exactly as it was (its reason and since; restored at
// revision 1, released), the start holds nothing of its own (its one pause record is the
// restore's) and records nothing: no applied record, no fact. SECOND is not held.
#[test]
fn s2_3_the_member_first_seen_under_another_pause_keeps_it_and_records_nothing() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    let host = start(&root, ids, &[]).expect("a start without the member");
    let receipt = pause(&host, instance_scope(FIRST), 4534);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    let operators = instance_pauses(&host)[FIRST].clone();
    assert_ne!(operators.pause_reason.as_str(), INSTANCE_ADDED_PAUSE_REASON);
    let pause_records = recorded(&host, PAUSES).len();
    host.close().expect("close the first host");

    let restarted = start(&root, ids, &[ids.0]).expect("the start that first sees the member");
    let pauses = instance_pauses(&restarted);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    let kept = &pauses[FIRST];
    assert_eq!(kept.pause_reason, operators.pause_reason, "its reason kept");
    assert_eq!(
        kept.since_unix_ms, operators.since_unix_ms,
        "its since kept"
    );
    assert_eq!(kept.revision, 1);
    assert_eq!(kept.stage, InstancePauseStage::Released);
    assert_eq!(
        recorded(&restarted, PAUSES).len(),
        pause_records + 1,
        "the restore's record alone"
    );
    assert_eq!(applied_rows(&restarted), None, "nothing applied");
    assert!(
        recorded(&restarted, APPLIED).is_empty(),
        "no applied record"
    );
    assert!(restarted.fatal_error().expect("runtime health").is_none());
    restarted.close().expect("close the restarted host");
}

// (4) A start that ends between the two records (its applied record refused after the pause row
// was recorded) fails; the next start finds FIRST's restored instance.added pause, keeps it and
// records the row; a third start finds the row and writes nothing of its own: the applied row is
// unchanged, and its one pause record is the restore's.
#[test]
fn s2_4_a_start_that_ends_between_the_two_records_is_completed_by_the_next_start_once() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    START_PAUSE_APPLIED_REFUSED_FOR_TEST.with(|refused| refused.set(true));
    let Err(failed) = start(&root, ids, &[ids.0]) else {
        panic!("the start ends between the two records");
    };
    assert_eq!(
        failed.code(),
        SanitizationCode::RuntimeFactRecordListTooLarge.as_str(),
        "{failed:?}"
    );
    assert!(
        !START_PAUSE_APPLIED_REFUSED_FOR_TEST.with(std::cell::Cell::get),
        "the refusal was taken"
    );

    let second = start(&root, ids, &[ids.0]).expect("the next start");
    let pauses = instance_pauses(&second);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    assert_held_added(&pauses[FIRST]);
    assert_eq!(applied_ids(&second), [id_text(ids.0)]);
    assert_eq!(recorded(&second, APPLIED).len(), 1);
    let rows = applied_rows(&second).expect("the applied row");
    let pause_records = recorded(&second, PAUSES).len();
    second.close().expect("close the second host");

    let third = start(&root, ids, &[ids.0]).expect("the third start");
    let pauses = instance_pauses(&third);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [FIRST.to_owned()],
        "{pauses:?}"
    );
    assert_held_added(&pauses[FIRST]);
    assert_eq!(
        applied_rows(&third),
        Some(rows),
        "the applied row unchanged"
    );
    assert_eq!(recorded(&third, APPLIED).len(), 1, "no applied record");
    assert_eq!(
        recorded(&third, PAUSES).len(),
        pause_records + 1,
        "the restore's record alone"
    );
    third.close().expect("close the third host");
}

// ---- A configuration without the member, compared with a host that never enters the code path.

/// The one parameter of the fixture's configuration manifest, at its default: a start with a
/// manifest records its configuration facts and `config.start`.
fn manifest() -> RuntimeConfigManifest {
    let mut parameters = vec![ConfigParameter {
        key: DEVICE_GROUP_SWITCH_FAILURE_COOLDOWN_MS.to_owned(),
        value: FactScalar::DurationMs(300_000),
        source: ConfigParameterSource::Default,
        settable: false,
    }];
    actingcommand_runtime_config::annotate_settable(&mut parameters)
        .expect("every key is in the key table");
    RuntimeConfigManifest {
        subsystems: Vec::new(),
        parameters,
    }
}

/// Whether a member is named `key`, whose value is a time or a place in the ledger: an instant on
/// either clock, a measurement on the process clock, a local day, a ledger position, revision or
/// record index, or the evaluation's `seed`. Two hosts hold these apart however alike their runs
/// are (the list of `gate_mapp2b`).
fn time_or_place(key: &str) -> bool {
    key.ends_with("_unix_ms")
        || key.ends_with("_monotonic_ms")
        || key.ends_with("_us")
        || key.ends_with("_at")
        || matches!(
            key,
            "task_timing"
                | "package_load_ms"
                | "returned_bytes"
                | "successful_returned_bytes"
                | "local_day"
                | "sequence"
                | "record_index"
                | "input_ledger_position"
                | "input_fact_revisions"
                | "seed"
        )
}

/// `run` (lowercase hexadecimal digits) as [`without_identities`] writes it: a run of 32 or more
/// is an identity and is replaced.
fn push_hex_run(text: &mut String, run: &mut String) {
    if run.len() >= 32 {
        text.push_str("<hex>");
    } else {
        text.push_str(run);
    }
    run.clear();
}

/// `text` with each run of 32 or more lowercase hexadecimal digits replaced: a minted id (a
/// request, run, lease, instance or event id, an owner epoch), a snapshot id, a digest.
fn without_identities(text: &str) -> String {
    let mut written = String::with_capacity(text.len());
    let mut run = String::new();
    for character in text.chars() {
        if matches!(character, '0'..='9' | 'a'..='f') {
            run.push(character);
        } else {
            push_hex_run(&mut written, &mut run);
            written.push(character);
        }
    }
    push_hex_run(&mut written, &mut run);
    written
}

/// The texts of `root`'s path a record may hold: as given, canonical, with forward slashes, and
/// its last component alone (the start record's `audit.machine_path` names the state root by its
/// base name: two temporary roots never share it); the longest first.
fn root_texts(root: &TempDir) -> Vec<String> {
    let mut texts = vec![root.path().display().to_string()];
    if let Ok(canonical) = root.path().canonicalize() {
        texts.push(canonical.display().to_string());
    }
    let slashed = texts
        .iter()
        .map(|text| text.replace('\\', "/"))
        .collect::<Vec<_>>();
    texts.extend(slashed);
    if let Some(name) = root.path().file_name() {
        texts.push(name.to_string_lossy().into_owned());
    }
    texts.sort_by_key(|text| std::cmp::Reverse(text.len()));
    texts
}

/// `value` with its ids, times and places normalised: every member [`time_or_place`] names is
/// replaced whole, every number is replaced (a measured value or a count of the process clock's),
/// and every string loses its root path and its identities. Everything else stays.
fn normalised(value: &mut Value, roots: &[String]) {
    if value.is_number() {
        *value = json!("<number>");
        return;
    }
    match value {
        Value::Object(members) => {
            for (key, member) in members.iter_mut() {
                if time_or_place(key) {
                    *member = json!("<normalised>");
                } else {
                    normalised(member, roots);
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                normalised(item, roots);
            }
        }
        Value::String(text) => {
            let mut written = text.clone();
            for root in roots {
                written = written.replace(root.as_str(), "<root>");
            }
            *text = without_identities(&written);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// One record as the comparison reads it: its type, its severity and its whole payload,
/// normalised ([`normalised`]).
#[derive(Debug, PartialEq)]
struct Compared {
    event_type: EventType,
    severity: EventSeverity,
    payload: Value,
}

/// The records of `events`, normalised against `roots`.
fn compared(events: &[PersistedEvent], roots: &[String]) -> Vec<Compared> {
    events
        .iter()
        .map(|event| {
            let mut payload =
                serde_json::to_value(event.payload()).expect("a record's payload encodes");
            normalised(&mut payload, roots);
            Compared {
                event_type: event.event_type(),
                severity: event.severity(),
                payload,
            }
        })
        .collect()
}

/// What one run of a configuration without the member left: the start's records (up to the
/// ledger's head when the start returned), the records of the release steps after it, and the
/// steps' receipts, each normalised.
struct WithoutMember {
    start: Vec<Compared>,
    releases: Vec<Compared>,
    receipts: Vec<Value>,
}

/// A configuration without the member on a new root, FIRST alone (`first`) on a default fake
/// device, with the configuration manifest: the start, then FIRST paused by an operator and
/// resumed by a person, then paused with the installer's hold and released by the installer.
/// With `off` the start's hold and the release's hold return at once on this thread, as a build
/// without the slice's code path ([`START_PAUSE_OFF_FOR_TEST`]).
fn without_member(first: InstanceId, off: bool) -> WithoutMember {
    START_PAUSE_OFF_FOR_TEST.with(|switch| switch.set(off));
    let root = TempDir::new().expect("tempdir");
    let host = RuntimeHost::start(
        config(&root).with_config_manifest(manifest()),
        Arc::new(FakeProvider::one(
            FIRST,
            first,
            Arc::new(FakeState::default()),
        )),
    )
    .expect("a start without the member");
    let started = head(&host);
    let roots = root_texts(&root);
    let mut receipts = Vec::new();
    for (release, connection) in [(Release::Person, 4570), (Release::Installer, 4572)] {
        let paused = pause_first(&host, release.reason(), connection);
        assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
        let released = release_first(&host, release, connection + 1);
        assert_eq!(
            released.state(),
            RuntimeReceiptState::Completed,
            "{released:?}"
        );
        for receipt in [paused, released] {
            let mut answer = serde_json::to_value(&receipt).expect("a receipt encodes");
            normalised(&mut answer, &roots);
            receipts.push(answer);
        }
    }
    assert!(paused_aliases(&host).is_empty(), "resumed");
    assert_eq!(applied_rows(&host), None, "no fact");
    let events = host
        .query_persisted_events_for_test(EventQuery::default())
        .expect("the root's records");
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the host without the member");
    START_PAUSE_OFF_FOR_TEST.with(|switch| switch.set(false));
    let (at_start, after_start): (Vec<_>, Vec<_>) = events
        .into_iter()
        .partition(|event| event.sequence() <= started);
    WithoutMember {
        start: compared(&at_start, &roots),
        releases: compared(&after_start, &roots),
        receipts,
    }
}

/// Every index at which `with` (the slice's code path entered) and `without` (never entered)
/// differ, each named with both records, so one message names them all.
fn differing<T: std::fmt::Debug + PartialEq>(what: &str, with: &[T], without: &[T]) -> Vec<String> {
    (0..with.len().max(without.len()))
        .filter(|index| with.get(*index) != without.get(*index))
        .map(|index| {
            format!(
                "{what} #{index}: with the code path {:#?}, without {:#?}",
                with.get(index),
                without.get(index)
            )
        })
        .collect()
}

// (5) A configuration without the member, as the restart test of `lifecycle.rs`
// (`scheduling_pauses_survive_a_restart_until_resumed`) finds a new root: nothing restored, no
// pause, no `host.start_pause_applied` held or recorded and no `host.scheduling_pause` recorded, at
// the first start and at a restart. Fix 1 (review item 5): the whole start of such a
// configuration, on a fixture with a configuration manifest, against the same start on a host of
// this build that never enters the slice's code path (the test-only switch): the same records,
// record by record, type, severity and whole payload with ids, times, numbers and the root path
// normalised (the evaluation's `seed` among the times), every differing record named in one
// message. Two hosts, the same instance id given to both; nothing else is compared between them.
#[test]
fn s2_5_a_configuration_without_the_member_holds_nothing_and_records_nothing_more() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    let host = start(&root, ids, &[]).expect("a start without the member");
    assert!(host.scheduling_pause_restore().is_empty());
    assert!(paused_aliases(&host).is_empty());
    assert_eq!(applied_rows(&host), None);
    assert!(recorded(&host, APPLIED).is_empty());
    assert!(recorded(&host, PAUSES).is_empty());
    host.close().expect("close the first host");

    let restarted = start(&root, ids, &[]).expect("a restart without the member");
    assert!(restarted.scheduling_pause_restore().is_empty());
    assert!(paused_aliases(&restarted).is_empty());
    assert_eq!(applied_rows(&restarted), None);
    assert!(recorded(&restarted, APPLIED).is_empty());
    assert!(recorded(&restarted, PAUSES).is_empty());
    restarted.close().expect("close the restarted host");

    let first = instance_id();
    let entered = without_member(first, false);
    let never = without_member(first, true);
    assert!(!entered.start.is_empty(), "the start has records");
    assert!(
        entered
            .start
            .iter()
            .any(|record| record.payload.to_string().contains("config.start")),
        "the fixture's manifest is recorded: {:#?}",
        entered.start
    );
    let differences = differing("start record", &entered.start, &never.start);
    assert!(
        differences.is_empty(),
        "the start's records are the same on both hosts; these differ:\n{}",
        differences.join("\n")
    );
}

// (6) Fix 1 (review item 1): the applied rows stay bounded. KEEP declares the member from the first
// start, which holds it; a person resumes it. Then, over 17 starts, 16 new instance ids at a time
// declare the member beside KEEP, each batch configured at one start only (273 distinct ids in
// all, more than a record list's 256 rows): no start fails, every new entry is held with
// instance.added, KEEP is never held again, and the fact's rows are exactly the configured
// instances that declared the member, never more than the configured instances.
#[test]
fn s2_6_the_applied_rows_keep_only_the_configured_instances_over_more_than_256_ids() {
    const STARTS: usize = 17;
    const NEW_PER_START: usize = 16;
    let root = TempDir::new().expect("tempdir");
    let keep = instance_id();
    let host = start_entries(&root, &[(KEEP.to_owned(), keep)], &BTreeSet::from([keep]))
        .expect("the first start");
    assert_eq!(paused_aliases(&host), [KEEP.to_owned()]);
    assert_eq!(applied_ids(&host), [id_text(keep)]);
    let receipt = resume(&host, instance_scope(KEEP), 4540);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    host.close().expect("close the first host");

    let mut declared_ever = BTreeSet::from([keep]);
    for round in 0..STARTS {
        let added = (0..NEW_PER_START)
            .map(|slot| (format!("added.{round}.{slot}"), instance_id()))
            .collect::<Vec<_>>();
        let mut entries = vec![(KEEP.to_owned(), keep)];
        entries.extend(added.iter().cloned());
        let declared = entries.iter().map(|(_, id)| *id).collect::<BTreeSet<_>>();
        declared_ever.extend(declared.iter().copied());
        let host = start_entries(&root, &entries, &declared)
            .unwrap_or_else(|error| panic!("start {round} fails: {error:?}"));
        let pauses = instance_pauses(&host);
        assert_eq!(
            pauses.keys().cloned().collect::<BTreeSet<_>>(),
            added
                .iter()
                .map(|(alias, _)| alias.clone())
                .collect::<BTreeSet<_>>(),
            "start {round}: the new entries are held, KEEP is not"
        );
        for pause in pauses.values() {
            assert_held_added(pause);
        }
        let mut rows = applied_ids(&host);
        rows.sort();
        let mut configured = declared.iter().map(|id| id_text(*id)).collect::<Vec<_>>();
        configured.sort();
        assert_eq!(
            rows, configured,
            "start {round}: the rows are the configured instances that declared the member"
        );
        assert!(rows.len() <= entries.len(), "start {round}: {rows:?}");
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close()
            .unwrap_or_else(|error| panic!("close start {round}: {error:?}"));
    }
    assert!(
        declared_ever.len() > 256,
        "more instance ids declared the member than a record list holds: {}",
        declared_ever.len()
    );
}

// (7) Fix 1 (the coordinator's ruling: the member is applied only by its own pause), for a person's
// resume and for the installer's release of its hold. FIRST, paused by that party on a host
// without the member, has its emulator started under the pause, so its startup claim waits gated
// behind the pause (W2-B keeps it across the restart). The start that first sees the member keeps
// that pause (reason, revision 1) and records nothing. The release: the receipt is
// `SchedulingResumed` with the new pause's revision 3 (lifted at 2, held at 3) and a self-check of
// both sides `ok: false` with `failure_code` `dispatch_paused_instance`; FIRST is held with
// instance.added, stage released; one pause record then one applied record after the release, the
// row naming FIRST. The scheduler: no lease is active on FIRST and its startup claim still waits
// gated, also after a sweep tick, and no task was requested, so nothing was dispatched at or after
// the release. Then a person resumes the instance.added pause: an ordinary resume, FIRST is not
// paused and its startup package runs once; no second applied record. A restart holds nothing and
// reads the same row.
#[test]
fn s2_7_the_release_of_another_pause_holds_an_unapplied_entry_with_instance_added() {
    for release in [Release::Person, Release::Installer] {
        let root = TempDir::new().expect("tempdir");
        let state = Arc::new(FakeState::default());
        let first = instance_id();
        let host = startup_start(&root, first, &state, false);
        let paused = pause_first(&host, release.reason(), 4560);
        assert_eq!(paused.state(), RuntimeReceiptState::Completed, "{paused:?}");
        let started = control(&host, FIRST, START, 4561);
        assert_eq!(
            started.state(),
            RuntimeReceiptState::Completed,
            "{started:?}"
        );
        host.close().expect("close the host without the member");

        let host = startup_start(&root, first, &state, true);
        let kept = instance_pauses(&host)[FIRST].clone();
        assert_eq!(kept.pause_reason.as_str(), release.reason(), "{release:?}");
        assert_eq!(kept.revision, 1, "{release:?}");
        assert_eq!(applied_rows(&host), None, "{release:?}: nothing applied");
        assert!(recorded(&host, APPLIED).is_empty(), "{release:?}");
        host.expire_due_leases_for_test().expect("sweep tick");
        assert_eq!(
            host.instance_claims_for_test(FIRST).expect("claims"),
            (None, vec![ClaimKind::StartupPackage]),
            "{release:?}: the startup claim waits behind the other pause"
        );

        let mark = head(&host);
        let receipt = release_first(&host, release, 4562);
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{release:?}: {receipt:?}"
        );
        let Some(RuntimeResult::SchedulingResumed {
            revision,
            selfcheck: Some(selfcheck),
            ..
        }) = receipt.result()
        else {
            panic!("{release:?}: a resume's answer: {receipt:?}");
        };
        let held = instance_pauses(&host)[FIRST].clone();
        assert_eq!(
            held.pause_reason.as_str(),
            INSTANCE_ADDED_PAUSE_REASON,
            "{release:?}"
        );
        assert_eq!(held.stage, InstancePauseStage::Released, "{release:?}");
        assert_eq!(held.revision, 3, "{release:?}: lifted at 2, held at 3");
        assert_eq!(
            *revision, held.revision,
            "{release:?}: the answer's revision"
        );
        assert_eq!(
            selfcheck.failure_code.as_deref(),
            Some(HostCode::DispatchPausedInstance.as_str()),
            "{release:?}: {selfcheck:?}"
        );
        assert!(
            !selfcheck.capture.ok && !selfcheck.touch.ok,
            "{release:?}: no reconnect: {selfcheck:?}"
        );
        assert_eq!(applied_ids(&host), [id_text(first)], "{release:?}");
        let pause_records = recorded_after(&host, PAUSES, mark);
        let applied_records = recorded_after(&host, APPLIED, mark);
        let ([pause_record], [applied_record]) = (&pause_records[..], &applied_records[..]) else {
            panic!(
                "{release:?}: one pause record and one applied record: {pause_records:?} \
                 {applied_records:?}"
            );
        };
        assert!(
            pause_record < applied_record,
            "{release:?}: the pause first, the applied row second"
        );
        assert_eq!(
            host.instance_claims_for_test(FIRST).expect("claims"),
            (None, vec![ClaimKind::StartupPackage]),
            "{release:?}: no lease, the startup claim still gated"
        );
        host.expire_due_leases_for_test().expect("sweep tick");
        assert_eq!(
            host.instance_claims_for_test(FIRST).expect("claims"),
            (None, vec![ClaimKind::StartupPackage]),
            "{release:?}: still gated after a sweep tick"
        );
        assert_eq!(
            records_of(&host, EventType::TaskRequested),
            0,
            "{release:?}: nothing dispatched"
        );

        let resumed_at = head(&host);
        let resumed = resume(&host, instance_scope(FIRST), 4563);
        assert_eq!(
            resumed.state(),
            RuntimeReceiptState::Completed,
            "{release:?}: {resumed:?}"
        );
        assert!(paused_aliases(&host).is_empty(), "{release:?}: resumed");
        await_recorded(&host, EventType::TaskCompleted, resumed_at);
        assert_eq!(
            records_of(&host, EventType::TaskRequested),
            1,
            "{release:?}: the startup package ran once"
        );
        let rows = applied_rows(&host).expect("the applied row");
        assert_eq!(recorded(&host, APPLIED).len(), 1, "{release:?}");
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close the host with the member");

        let host = startup_start(&root, first, &state, true);
        assert!(
            paused_aliases(&host).is_empty(),
            "{release:?}: never held again"
        );
        assert_eq!(applied_rows(&host), Some(rows), "{release:?}");
        assert_eq!(recorded(&host, APPLIED).len(), 1, "{release:?}");
        assert!(host.fatal_error().expect("runtime health").is_none());
        host.close().expect("close the restarted host");
    }
}

// (8) Fix 1: a start that finds the earlier other pause gone holds instance.added then. FIRST,
// paused by an operator, keeps that pause at the start that first sees the member (nothing
// recorded). The next start configures the same instance id under another alias, so the restore
// drops the pause of FIRST's alias, and the entry, not paused now, is held with instance.added and
// recorded after its pause.
#[test]
fn s2_8_a_start_that_finds_the_other_pause_gone_holds_the_entry_then() {
    let root = TempDir::new().expect("tempdir");
    let ids = (instance_id(), instance_id());
    let host = start(&root, ids, &[]).expect("a start without the member");
    let receipt = pause(&host, instance_scope(FIRST), 4580);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    host.close().expect("close the first host");

    let host = start(&root, ids, &[ids.0]).expect("the start that first sees the member");
    assert_eq!(paused_aliases(&host), [FIRST.to_owned()]);
    assert_ne!(
        instance_pauses(&host)[FIRST].pause_reason.as_str(),
        INSTANCE_ADDED_PAUSE_REASON
    );
    assert_eq!(applied_rows(&host), None);
    host.close().expect("close the second host");

    let host = start_entries(
        &root,
        &[(RENAMED.to_owned(), ids.0), (SECOND.to_owned(), ids.1)],
        &BTreeSet::from([ids.0]),
    )
    .expect("the start that finds the pause gone");
    assert_eq!(
        reported(
            host.scheduling_pause_restore(),
            HostCode::SchedulingPauseDropped.as_str()
        ),
        1,
        "the restore dropped FIRST's pause: {:?}",
        host.scheduling_pause_restore()
    );
    let pauses = instance_pauses(&host);
    assert_eq!(
        pauses.keys().cloned().collect::<Vec<_>>(),
        [RENAMED.to_owned()],
        "{pauses:?}"
    );
    assert_held_added(&pauses[RENAMED]);
    assert_eq!(applied_ids(&host), [id_text(ids.0)]);
    let pause_records = recorded(&host, PAUSES);
    let applied_records = recorded(&host, APPLIED);
    assert_eq!(applied_records.len(), 1, "{applied_records:?}");
    assert!(
        pause_records.last() < applied_records.first(),
        "{pause_records:?} {applied_records:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    host.close().expect("close the third host");
}

// (9) Fix 1: the release paths of a configuration without the member, a person's resume and the
// installer's release (each after its party's pause), compared with the same steps on a host of
// this build that never enters the slice's code path (the test-only switch): the same records,
// record by record, and the same receipts, with ids, times, numbers and the root path normalised,
// every differing record named in one message.
#[test]
fn s2_9_the_release_paths_without_the_member_are_those_of_a_host_without_the_code_path() {
    let first = instance_id();
    let entered = without_member(first, false);
    let never = without_member(first, true);
    assert!(!entered.releases.is_empty(), "the steps have records");
    let mut differing_records = differing("release record", &entered.releases, &never.releases);
    differing_records.extend(differing("receipt", &entered.receipts, &never.receipts));
    assert!(
        differing_records.is_empty(),
        "the release steps' records and receipts are the same on both hosts; these differ:\n{}",
        differing_records.join("\n")
    );
}
