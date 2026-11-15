// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #463 S3-11 (register §1.418; contracts/standard-pack-trial.md, "The run", "The end",
// "Reading the end later"; coordinator work order rd-1115/brief-463-s311-trial-run.md): `actinglab
// standard-pack trial` without `--plan` sends the trial's one run, reads its business end as `run
// show` reads it and records it. Each test drives the runtime client's scripted Runtime on loopback
// in a temporary state root and writes down every frame of every connection in the order the
// product sends them:
// - `s311_each_run_only_refusal_sends_nothing_and_writes_nothing`: no `--authorization`
//   (`trial_authorization_missing`), a package the mark flags over `--root` without
//   `--irreversible` (`trial_irreversible_unacknowledged`), a `--variable`
//   (`trial_variables_not_carried`), `--dry-run` (`dry_run_unsupported`), and the run's flags
//   beside the plan or each other (`conflicting`), a `--collect` that is no request id
//   (`not_recognised`) and a `--wait` that is no duration (`wrong_type`): no connection, no state
//   directory;
// - `s311_a_run_ending_on_the_target_page_records_its_key_and_matched`: the run sent once, then
//   its projection, its facts, the two restart reads and its terminal's record read; the answer's
//   end `succeeded` with the target's key and page, `expected` and `matched` true, no warning; the
//   record at `<state>/trials/<id>/<content12>-<terminal's time>.json`, its plan with the
//   authorization exactly as given, its end and `matched`;
// - `s311_a_run_ending_on_an_ending_page_succeeds_with_matched_false_and_a_warning`: exit 0,
//   `matched` false, the warning `trial_end_unexpected` naming the ending's key, the record keeping
//   that key;
// - `s311_a_failed_run_records_its_code_and_no_key`: the run's code and values on top, no key,
//   `matched` false, the record written with the code and no key;
// - `s311_an_unknown_result_sends_nothing_again_and_writes_no_record`: the connection closed after
//   the run's request left: the client's failure with the request id and the line that reads the
//   end later; nothing more on the connection and no record;
// - `s311_a_wait_that_runs_out_writes_no_record_and_collect_records_it_once`: `--wait 0ms` answers
//   `command_wait_expired` with the request id and `next`, writes nothing; that line, run against
//   the Runtime's ledger of that run, sends nothing, records the end once, and a second
//   `--collect` of it is `trial_record_exists`, one record in the directory;
// - `s311_collect_of_another_packages_run_is_refused`: `trial_collect_mismatch` `other_package`,
//   no record.
// No device, no clock read, no sleep: temporary directories, a made-up package, made-up ledgers.

use super::*;
use actingcommand_contract::outcome::Outcome;
use actingcommand_contract::{
    AuditInput, CorrelationId, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventQuery,
    EventSeverity, EventSource, IdentifierIssuer, InstanceId, IssuedInstanceId, OriginModule,
    OwnerEpoch, ProjectedEvent, ProjectionPayload, RequestId, RuntimeControlPlaneStatus,
    RuntimeErrorCode, RuntimeErrorProjection, RuntimeEventQueryPage, RuntimeInstanceStatus,
    RuntimeOperation, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    SanitizationError, SchedulingDisposition, SchedulingEffectEvidence, SecretField,
    SecretFingerprinter, Sha256Fingerprint, TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
    TerminalEvent,
};
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedConnectionRecord, ScriptedGate, ScriptedRuntime,
};
use std::sync::{Arc, Mutex, PoisonError};

/// The made-up package's id, and another package's.
const PACKAGE_ID: &str = "neutral.test.trial";
const OTHER_PACKAGE: &str = "neutral.test.other";
/// The scripted Runtime's one instance.
const ALIAS: &str = "neutral.trial";
const AUTHORIZATION: &str = "board item 463, comment 0000000003";
/// The keys of the target page and of the ending page.
const DONE_KEY: &str = "task_done";
const SHORT_KEY: &str = "task_short";
/// The made-up ledgers' first time, 2100-01-01, and one record a second from there on.
const START_UNIX_MS: u64 = 4_102_444_800_000;
const SPACING_MS: u64 = 1_000;
/// The requests of every connection's handshake.
const HANDSHAKE: [&str; 2] = ["health", "bind_channel"];

/// A context with no configuration and no recording session, the process's initiator word unset
/// and a Runtime state root for the scripted Runtime, beside the package, the state directory and
/// a game root. The fields drop in order, the environment lock last.
struct Context {
    files: TempDir,
    _runtime: RuntimeStateEnvGuard,
    _app_env: TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Context {
    fn new() -> Self {
        let guard = env_lock();
        let app_env = set_isolated_app_env();
        set_missing_config_env();
        unsafe {
            env::remove_var(SESSION_STATE_ENV);
            env::remove_var(ENVIRONMENT_VARIABLE);
        }
        let files = TempDir::new().unwrap();
        let runtime_root = files.path().join("runtime-state");
        fs::create_dir_all(&runtime_root).unwrap();
        let runtime = use_runtime_state_root(&runtime_root);
        let context = Self {
            files,
            _runtime: runtime,
            _app_env: app_env,
            _guard: guard,
        };
        context.write_package();
        context
    }

    fn path(&self, name: &str) -> PathBuf {
        self.files.path().join(name)
    }

    fn runtime_root(&self) -> PathBuf {
        self.path("runtime-state")
    }

    fn package(&self) -> PathBuf {
        self.path("packs").join(PACKAGE_ID)
    }

    fn state(&self) -> PathBuf {
        self.path("state")
    }

    /// The package [`PACKAGE_ID`] as a directory `packs/<id>`: a `linear_steps` package of one
    /// trusted click from `home` (a blue first pixel) to its target page `done` (white), its ending
    /// page `short` (green), the keys [`DONE_KEY`] and [`SHORT_KEY`], and the variable `floor`.
    fn write_package(&self) {
        let probe = |page: &str, expected: [u8; 3]| {
            json!({
                "id": format!("{page}_colour"),
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                "expected": expected
            })
        };
        let mapping = |key: &str, page: &str| json!({"outcome_key": key, "effect": "no_designated_effect", "terminal_pages": [page]});
        let control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": PACKAGE_ID,
            "execution_mode": "linear_steps",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 64, "height": 24},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 1,
            "variables": [{"name": "floor", "type": "integer", "title": {"en": "Keep at least"},
                           "min": 0, "max": 200, "step": 10, "default": 0}]
        });
        let task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "S3-11 fixture",
            "coordinate_space": {"width": 64, "height": 24},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": [
                probe("home", [0, 0, 255]),
                probe("done", [255, 255, 255]),
                probe("short", [0, 255, 0])
            ],
            "page_rules": {
                "home": {"required": ["home_colour"]},
                "done": {"required": ["done_colour"]},
                "short": {"required": ["short_colour"]}
            },
            "entry_page": "home",
            "target_page": "done",
            "max_steps": 1,
            "scheduling_outcome": {"mappings": [
                mapping(DONE_KEY, "done"),
                mapping(SHORT_KEY, "short")
            ]},
            "operations": [{
                "id": "go", "from": "home", "to": "done",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true, "retryable": false
            }]
        });
        write_files(&self.package(), control, task);
    }

    /// A game root `<files>/root/neutral` whose `bundle.json` registers another package alone, so
    /// that the package tried counts as marked (the type's safe side); its path.
    fn marking_root(&self) -> PathBuf {
        let root = self.path("root").join("neutral");
        let control = json!({
            "schema_version": "Lab-1y.control.v2",
            "package_id": OTHER_PACKAGE,
            "execution_mode": "navigable_route",
            "game": "neutral",
            "server": "test",
            "resolution": {"width": 16, "height": 9},
            "entry_task_id": "task",
            "capture_interval_ms": 10,
            "step_timeout_ms": 600,
            "timeout_ms": 8000,
            "max_steps": 2
        });
        let probe = |page: &str, expected: [u8; 3]| {
            json!({
                "id": format!("{page}_colour"),
                "region": {"mode": "rect", "rect": {"x": 0, "y": 0, "width": 1, "height": 1}},
                "expected": expected
            })
        };
        let task = json!({
            "schema_version": "0.9",
            "task_id": "task",
            "game": "neutral",
            "server_scope": ["test"],
            "locale": "en-US",
            "goal": "S3-11 fixture",
            "coordinate_space": {"width": 16, "height": 9},
            "defaults": {"template_threshold": 0.9, "color_max_distance": 0.0},
            "color_probes": [probe("home", [0, 0, 255]), probe("page_b", [255, 0, 0])],
            "page_rules": {
                "home": {"required": ["home_colour"]},
                "page_b": {"required": ["page_b_colour"]}
            },
            "entry_page": "home",
            "target_page": "page_b",
            "operations": [{
                "id": "go", "from": "home", "to": "page_b",
                "click": {"kind": "point", "x": 1, "y": 0},
                "unguarded_trusted_coordinate": true, "retryable": false
            }]
        });
        write_files(&root.join(OTHER_PACKAGE), control, task);
        let bundle = json!({"schema_version": "actingcommand.bundle.v4", "game": "neutral",
            "packs": [{"package_id": OTHER_PACKAGE, "server": "test", "entry_task_id": "task",
                       "path": format!("packs/{OTHER_PACKAGE}"), "file_count": 3,
                       "byte_count": 64}]});
        fs::write(
            root.join("bundle.json"),
            serde_json::to_vec_pretty(&bundle).unwrap(),
        )
        .unwrap();
        root
    }

    /// `actinglab --json standard-pack trial --instance <ALIAS> --package <the package>
    /// --state-dir <the state directory>`, then `extra`.
    fn line(&self, extra: &[&str]) -> CliResult {
        let package = self.package().display().to_string();
        let state = self.state().display().to_string();
        let mut words = vec![
            "--json",
            "standard-pack",
            "trial",
            "--instance",
            ALIAS,
            "--package",
            package.as_str(),
            "--state-dir",
            state.as_str(),
        ];
        words.extend_from_slice(extra);
        run_cli(words.into_iter().map(str::to_owned), true)
    }

    /// The trial records' directory of the package.
    fn trials(&self) -> PathBuf {
        std::path::absolute(self.state())
            .unwrap()
            .join("trials")
            .join(PACKAGE_ID)
    }
}

fn write_files(directory: &Path, control: Value, task: Value) {
    for (relative, value) in [
        ("control.json", control),
        (
            "resources/operations/resources.json",
            json!({"schema_version": "1.0", "resources": [], "resource_count": 0}),
        ),
        ("resources/operations/task/task.json", task),
    ] {
        let path = directory.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
}

/// The answer is the refusal `code` with the exit `exit` and no data; its details.
#[track_caller]
fn refused(result: &CliResult, code: &str, exit: i32) -> Value {
    let shown = result.envelope_json();
    assert_eq!(result.exit_code(), exit, "{shown}");
    assert!(result.envelope.data.is_none(), "{shown}");
    let error = result.envelope.error.as_ref().expect("a refusal");
    assert_eq!(error.code, code, "{shown}");
    error.details.clone().unwrap_or(Value::Null)
}

/// The answer is `validation_failed` naming `argument` as `problem`.
#[track_caller]
fn argument_refused(result: &CliResult, argument: &str, problem: &str) {
    let details = refused(result, "validation_failed", 2);
    assert_eq!(details["argument"], argument, "{details}");
    assert_eq!(details["argument_problem"], problem, "{details}");
}

/// The operation of each request a connection received.
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
fn frames(then: &[&str]) -> Vec<String> {
    HANDSHAKE
        .iter()
        .chain(then)
        .map(|operation| (*operation).to_owned())
        .collect()
}

/// The scripted status: the one instance [`ALIAS`].
fn status(
    owner_epoch: OwnerEpoch,
    instance_id: InstanceId,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let instance =
            RuntimeInstanceStatus::new(ALIAS, instance_id, false, 0, false, false, false)
                .expect("instance status");
        let status =
            RuntimeControlPlaneStatus::new(owner_epoch, vec![instance]).expect("scripted status");
        ScriptedAnswer::completed(request, RuntimeResult::Status { status })
    }
}

struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the made-up ledger holds no secrets")
    }
}

/// How a made-up run ends.
#[derive(Clone, Copy)]
enum End {
    /// Normally on the page of the key `(key, page)`.
    Keyed(&'static str, &'static str),
    /// Failed: its deadline exceeded, on `neutral/home`.
    Failed,
}

/// The ledger of one run of `package_id` on `instance` that ended `end`: its admission and its
/// terminal, one second apart from [`START_UNIX_MS`]. Its request id, and its correlation when
/// given, are `ids` (the client's: the links are rewritten to them, since the client mints its
/// own); else minted here.
fn run_ledger(
    instance: IssuedInstanceId,
    package_id: &str,
    end: End,
    ids: Option<(RequestId, Option<CorrelationId>)>,
) -> Vec<ProjectedEvent> {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let links = EventLinksDraft::default()
        .with_instance_id(instance)
        .with_request_id(issuer.mint_request_id().expect("request id"))
        .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
        .with_task_id(issuer.mint_task_id().expect("task id"))
        .with_run_id(issuer.mint_run_id().expect("run id"));
    let terminal = match end {
        End::Keyed(key, page) => TaskSemanticFact::TerminalCommitted {
            task_result: TaskOutcome::Success,
            final_page: Some(page.to_owned()),
            executed_steps: Some(1),
            outcome: None,
            failure_severe: false,
            scheduling_disposition: Some(
                SchedulingDisposition::new(key, SchedulingEffectEvidence::NoDesignatedEffect)
                    .expect("a disposition"),
            ),
            task_timing: None,
        },
        End::Failed => TaskSemanticFact::TerminalCommitted {
            task_result: TaskOutcome::Failure,
            final_page: Some("neutral/home".to_owned()),
            executed_steps: Some(0),
            outcome: Some(
                Outcome::new(RuntimeErrorCode::ContainedTaskDeadlineExceeded)
                    .receipt_state(RuntimeReceiptState::Failed),
            ),
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        },
    };
    let facts = [
        TaskSemanticFact::PackageAdmitted {
            package_label: package_id.to_owned(),
            task_label: "task".to_owned(),
            response_deadline_monotonic_ms: None,
        },
        terminal,
    ];
    let mut events = Vec::new();
    for (index, fact) in (0_u64..).zip(facts) {
        let sanitized = EventDraft::new(
            issuer.mint_event_id().expect("event id"),
            START_UNIX_MS + index * SPACING_MS,
            EventSeverity::Info,
            EventOrigin::new(
                EventSource::Runtime,
                OriginModule::Runtime,
                EventActor::Runtime,
            ),
            links.clone(),
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        )
        .sanitize(&NoSecrets)
        .expect("sanitize a made-up ledger event");
        let mut event = ProjectedEvent {
            views: Vec::new(),
            artifact_evictions: Vec::new(),
            schema_version: sanitized.schema_version().to_owned(),
            sequence: index + 1,
            event_id: *sanitized.event_id(),
            timestamp_unix_ms: sanitized.timestamp_unix_ms(),
            event_type: sanitized.event_type(),
            severity: sanitized.severity(),
            sensitivity: sanitized.sensitivity(),
            origin: sanitized.origin().clone(),
            links: sanitized.links().clone(),
            payload_schema: sanitized.payload_schema().to_owned(),
            payload: ProjectionPayload::Full(Box::new(sanitized.payload().clone())),
            artifacts: Vec::new(),
        };
        if let Some((request_id, correlation_id)) = ids {
            let mut relinked = serde_json::to_value(&event.links).expect("links as JSON");
            relinked["request_id"] = json!(request_id);
            if let Some(correlation_id) = correlation_id {
                relinked["correlation_id"] = json!(correlation_id);
            }
            event.links = serde_json::from_value(relinked).expect("links from JSON");
        }
        events.push(event);
    }
    events
}

/// The request id of the run of `events`, as text.
fn request_id_of(events: &[ProjectedEvent]) -> String {
    json!(events[0].links.request_id().expect("a request id"))
        .as_str()
        .expect("a request id is text")
        .to_owned()
}

/// Whether `query` selects `event`: the links, the types and the sequence bounds the trial's reads
/// name.
fn selects(query: &EventQuery, event: &ProjectedEvent) -> bool {
    let links = &event.links;
    query
        .request_id
        .is_none_or(|id| links.request_id() == Some(&id))
        && query
            .correlation_id
            .is_none_or(|id| links.correlation_id() == Some(&id))
        && query
            .event_type
            .is_none_or(|event_type| event.event_type == event_type)
        && (query.event_types.is_empty() || query.event_types.contains(&event.event_type))
        && query
            .from_sequence
            .is_none_or(|from| event.sequence >= from)
        && query.to_sequence.is_none_or(|to| event.sequence <= to)
}

/// One page of `events` for the event query `request`, at one snapshot (the last sequence).
fn page_of(events: &[ProjectedEvent], request: &RuntimeRequest) -> ScriptedAnswer {
    let RuntimeOperation::QueryEvents { query, page, .. } = request.operation() else {
        panic!("an event query, got {:?}", request.operation())
    };
    let snapshot = events.last().map_or(0, |event| event.sequence);
    if let Some(position) = page.snapshot_position() {
        assert_eq!(position, snapshot, "a read at another snapshot");
    }
    let after = page.cursor().map_or(0, |cursor| cursor.after_sequence());
    let rows = events
        .iter()
        .filter(|event| event.sequence > after && selects(query, event))
        .take(usize::from(page.limit()))
        .cloned()
        .collect::<Vec<_>>();
    let page = RuntimeEventQueryPage::new(rows, snapshot, page.limit(), false, None)
        .expect("an event page");
    ScriptedAnswer::completed(request, RuntimeResult::EventPage { page })
}

/// Answers one event query from the ledger the run's answer left in `ledger`.
fn page_from(
    ledger: Arc<Mutex<Vec<ProjectedEvent>>>,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let events = ledger
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        page_of(&events, request)
    }
}

/// Answers the trial's run as it ended `end`, its ledger linked to the run's own request left in
/// `ledger`: a completed receipt for a run that ended normally, a failed one with its terminal for
/// a failed run.
fn run_answered(
    ledger: Arc<Mutex<Vec<ProjectedEvent>>>,
    instance: IssuedInstanceId,
    end: End,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        assert!(
            matches!(
                request.operation(),
                RuntimeOperation::RunContainedTask { .. }
            ),
            "the run, got {:?}",
            request.operation()
        );
        let events = run_ledger(
            instance,
            PACKAGE_ID,
            end,
            Some((request.request_id(), Some(request.correlation_id()))),
        );
        let terminal = events.last().expect("the terminal").clone();
        *ledger.lock().unwrap_or_else(PoisonError::into_inner) = events;
        match end {
            End::Keyed(_, page) => ScriptedAnswer::completed(
                request,
                RuntimeResult::ContainedTaskCompleted {
                    run_id: *terminal.links.run_id().expect("a run id"),
                    task_id: *terminal.links.task_id().expect("a task id"),
                    task_request_id: request.request_id(),
                    response_deadline_monotonic_ms: None,
                    task_result: TaskOutcome::Success,
                    final_page: Some(page.to_owned()),
                    executed_steps: 1,
                },
            ),
            End::Failed => ScriptedAnswer::Receipt(Box::new(
                RuntimeReceipt::failure(
                    request,
                    RuntimeReceiptState::Failed,
                    Some(TerminalEvent {
                        sequence: terminal.sequence,
                        event_id: terminal.event_id,
                    }),
                    RuntimeErrorProjection::new(
                        RuntimeErrorCode::ContainedTaskDeadlineExceeded,
                        false,
                    )
                    .receipt_outcome(None, None),
                )
                .expect("a failed receipt"),
            )),
        }
    }
}

/// The script of a trial's connection whose run ends `end`: the status read, the run, then the
/// five event reads the product sends after it: the receipt's projection (by its correlation),
/// the run's facts (by its request), the two restart reads (a start and a takeover) and the read
/// of its terminal's record.
fn sent_run(owner_epoch: OwnerEpoch, instance: IssuedInstanceId, end: End) -> ScriptedConnection {
    let ledger = Arc::new(Mutex::new(Vec::new()));
    let mut connection = ScriptedConnection::new()
        .request(status(owner_epoch, *instance.transport()))
        .request(run_answered(Arc::clone(&ledger), instance, end));
    for _ in 0..5 {
        connection = connection.request(page_from(Arc::clone(&ledger)));
    }
    connection
}

/// The frames of [`sent_run`].
fn sent_frames() -> Vec<String> {
    frames(&[
        "status",
        "run_contained_task",
        "query_events",
        "query_events",
        "query_events",
        "query_events",
        "query_events",
    ])
}

/// The request id of the run the connection `record` sent, as text.
fn sent_request_id(record: &ScriptedConnectionRecord) -> String {
    let run = record
        .requests
        .iter()
        .find(|request| {
            matches!(
                request.operation(),
                RuntimeOperation::RunContainedTask { .. }
            )
        })
        .expect("the run's request");
    json!(run.request_id())
        .as_str()
        .expect("a request id is text")
        .to_owned()
}

/// The record file `path`, read.
fn record_at(path: &Value) -> Value {
    let path = path.as_str().expect("the record's path");
    serde_json::from_slice(&fs::read(path).expect("the record file")).expect("the record reads")
}

/// The record's file name of the plan `plan` for a run whose terminal was recorded second.
fn record_name(plan: &Value) -> String {
    format!(
        "{}-{}.json",
        plan["content_hash_prefix"]
            .as_str()
            .expect("the content prefix"),
        START_UNIX_MS + SPACING_MS
    )
}

#[test]
fn s311_each_run_only_refusal_sends_nothing_and_writes_nothing() {
    let context = Context::new();
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |_| Vec::new());
    let root = context.marking_root().display().to_string();

    let details = refused(&context.line(&[]), "trial_authorization_missing", 2);
    assert_eq!(
        details["outcome"]["values"],
        json!({"package_id": PACKAGE_ID})
    );
    let details = refused(
        &context.line(&["--authorization", AUTHORIZATION, "--root", root.as_str()]),
        "trial_irreversible_unacknowledged",
        2,
    );
    assert_eq!(
        details["outcome"]["values"],
        json!({"package_id": PACKAGE_ID})
    );
    let details = refused(
        &context.line(&["--authorization", AUTHORIZATION, "--variable", "floor=20"]),
        "trial_variables_not_carried",
        2,
    );
    assert_eq!(
        details["outcome"]["values"],
        json!({"package_id": PACKAGE_ID, "variable_name": "floor"})
    );
    // The global `--dry-run`: the run and `--collect` are refused after the flags.
    let package = context.package().display().to_string();
    let state = context.state().display().to_string();
    let dry = run_cli(
        [
            "--json",
            "--dry-run",
            "standard-pack",
            "trial",
            "--instance",
            ALIAS,
            "--package",
            package.as_str(),
            "--authorization",
            AUTHORIZATION,
            "--state-dir",
            state.as_str(),
        ],
        true,
    );
    refused(&dry, "dry_run_unsupported", 2);
    // The run's flags.
    argument_refused(
        &context.line(&["--plan", "--collect", "req-1"]),
        "--collect",
        "conflicting",
    );
    argument_refused(
        &context.line(&["--plan", "--wait", "5s"]),
        "--wait",
        "conflicting",
    );
    argument_refused(
        &context.line(&["--collect", "req-1", "--wait", "5s"]),
        "--wait",
        "conflicting",
    );
    argument_refused(
        &context.line(&["--authorization", AUTHORIZATION, "--collect", "req-1"]),
        "--collect",
        "not_recognised",
    );
    argument_refused(
        &context.line(&["--authorization", AUTHORIZATION, "--wait", "soon"]),
        "--wait",
        "wrong_type",
    );

    let record = runtime.finish();
    assert!(record.connections.is_empty());
    assert!(
        record.unscripted.is_empty(),
        "a refused line connected: {:?}",
        record.unscripted
    );
    assert!(
        !context.state().exists(),
        "a refused line made the state directory"
    );
}

#[test]
fn s311_a_run_ending_on_the_target_page_records_its_key_and_matched() {
    let context = Context::new();
    let instance = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id");
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |owner_epoch| {
        vec![sent_run(
            owner_epoch,
            instance,
            End::Keyed(DONE_KEY, "neutral/done"),
        )]
    });
    let answered = context.line(&["--authorization", AUTHORIZATION, "--expect", DONE_KEY]);
    let record = runtime.finish();
    assert_eq!(operations(&record.connections[0]), sent_frames());
    assert!(record.unscripted.is_empty());

    let shown = answered.envelope_json();
    assert_eq!(answered.exit_code(), 0, "{shown}");
    let data = answered.envelope.data.clone().expect("the answer");
    let request_id = sent_request_id(&record.connections[0]);
    assert_eq!(data["sent"], true, "{shown}");
    assert_eq!(data["request_id"], request_id.as_str(), "{shown}");
    assert!(data["run_id"].is_string(), "{shown}");
    assert_eq!(
        data["end"],
        json!({"state": "succeeded", "outcome_key": DONE_KEY, "final_page": "neutral/done"}),
        "{shown}"
    );
    assert_eq!(data["expected"], DONE_KEY, "{shown}");
    assert_eq!(data["matched"], true, "{shown}");
    assert!(data.get("warnings").is_none(), "{shown}");
    assert_eq!(data["plan"]["authorization"], AUTHORIZATION, "{shown}");
    let place = context.trials().join(record_name(&data["plan"]));
    assert_eq!(
        data["record"],
        place.display().to_string().as_str(),
        "{shown}"
    );

    let kept = record_at(&data["record"]);
    assert_eq!(kept["schema_version"], "actingcommand.pack-trial.v1");
    assert_eq!(kept["at_unix_ms"], START_UNIX_MS + SPACING_MS);
    assert_eq!(kept["plan"]["authorization"], AUTHORIZATION);
    assert_eq!(kept["plan"]["expect"], DONE_KEY);
    assert_eq!(kept["request_id"], request_id.as_str());
    assert_eq!(kept["run_id"], data["run_id"]);
    assert_eq!(
        kept["end"],
        json!({"outcome": "success", "outcome_key": DONE_KEY, "final_page": "neutral/done"})
    );
    assert_eq!(kept["matched"], true);
}

#[test]
fn s311_a_run_ending_on_an_ending_page_succeeds_with_matched_false_and_a_warning() {
    let context = Context::new();
    let instance = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id");
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |owner_epoch| {
        vec![sent_run(
            owner_epoch,
            instance,
            End::Keyed(SHORT_KEY, "neutral/short"),
        )]
    });
    let answered = context.line(&["--authorization", AUTHORIZATION, "--expect", DONE_KEY]);
    let record = runtime.finish();
    assert_eq!(operations(&record.connections[0]), sent_frames());

    let shown = answered.envelope_json();
    assert_eq!(answered.exit_code(), 0, "{shown}");
    let data = answered.envelope.data.clone().expect("the answer");
    assert_eq!(
        data["end"],
        json!({"state": "succeeded", "outcome_key": SHORT_KEY, "final_page": "neutral/short"}),
        "{shown}"
    );
    assert_eq!(data["expected"], DONE_KEY, "{shown}");
    assert_eq!(data["matched"], false, "{shown}");
    let warnings = data["warnings"].as_array().cloned().unwrap_or_default();
    assert_eq!(warnings.len(), 1, "{shown}");
    assert_eq!(warnings[0]["code"], "trial_end_unexpected", "{shown}");
    assert_eq!(warnings[0]["category"], "warning", "{shown}");
    assert_eq!(
        warnings[0]["values"],
        json!({"package_id": PACKAGE_ID, "outcome_key": SHORT_KEY}),
        "{shown}"
    );
    let kept = record_at(&data["record"]);
    assert_eq!(kept["end"]["outcome_key"], SHORT_KEY);
    assert_eq!(kept["matched"], false);
    assert_eq!(kept["plan"]["authorization"], AUTHORIZATION);
}

#[test]
fn s311_a_failed_run_records_its_code_and_no_key() {
    let context = Context::new();
    let instance = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id");
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |owner_epoch| {
        vec![sent_run(owner_epoch, instance, End::Failed)]
    });
    let answered = context.line(&["--authorization", AUTHORIZATION, "--expect", DONE_KEY]);
    let record = runtime.finish();
    assert_eq!(operations(&record.connections[0]), sent_frames());

    let details = refused(&answered, "contained_task_deadline_exceeded", 4);
    let failed_end = json!({
        "state": "failed",
        "final_page": "neutral/home",
        "failure_code": "contained_task_deadline_exceeded",
        "failure_values": {"receipt_state": "failed"}
    });
    assert_eq!(details["end"], failed_end, "{details}");
    assert_eq!(details["sent"], true, "{details}");
    assert_eq!(
        details["request_id"],
        sent_request_id(&record.connections[0]).as_str(),
        "{details}"
    );
    assert_eq!(details["matched"], false, "{details}");
    assert_eq!(
        details["outcome"],
        json!({"code": "contained_task_deadline_exceeded", "values": {"receipt_state": "failed"}}),
        "{details}"
    );
    let kept = record_at(&details["record"]);
    assert_eq!(kept["end"]["outcome"], "failure");
    assert_eq!(
        kept["end"]["failure_code"],
        "contained_task_deadline_exceeded"
    );
    assert!(kept["end"].get("outcome_key").is_none(), "{kept}");
    assert_eq!(kept["matched"], false);
}

#[test]
fn s311_an_unknown_result_sends_nothing_again_and_writes_no_record() {
    let context = Context::new();
    let instance_id = *IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id")
        .transport();
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .request(|_| ScriptedAnswer::Close),
        ]
    });
    let answered = context.line(&["--authorization", AUTHORIZATION]);
    let record = runtime.finish();
    assert_eq!(
        operations(&record.connections[0]),
        frames(&["status", "run_contained_task"]),
        "a request after the run's"
    );
    assert!(record.unscripted.is_empty());

    let shown = answered.envelope_json();
    assert_eq!(answered.exit_code(), 4, "{shown}");
    let error = answered.envelope.error.as_ref().expect("a refusal");
    assert!(!error.code.starts_with("trial_"), "{shown}");
    let details = error.details.clone().expect("the details");
    let request_id = sent_request_id(&record.connections[0]);
    assert_eq!(details["request_id"], request_id.as_str(), "{shown}");
    assert_eq!(details["sent"], true, "{shown}");
    let next = details["next"]["collect"]
        .as_array()
        .cloned()
        .expect("the line that reads the end later");
    assert_eq!(
        next[next.len() - 2..],
        [json!("--collect"), json!(request_id)],
        "{shown}"
    );
    assert!(!context.trials().exists(), "a record was written");
}

#[test]
fn s311_a_wait_that_runs_out_writes_no_record_and_collect_records_it_once() {
    let context = Context::new();
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    let instance = issuer.mint_instance_id().expect("instance id");
    let instance_id = *instance.transport();
    let run_id = *issuer.mint_run_id().expect("run id").transport();
    let task_id = *issuer.mint_task_id().expect("task id").transport();
    let release = ScriptedGate::new();

    // The run answers only once the trial stopped waiting.
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |owner_epoch| {
        let release = release.clone();
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .request(move |request| {
                    ScriptedAnswer::completed(
                        request,
                        RuntimeResult::ContainedTaskCompleted {
                            run_id,
                            task_id,
                            task_request_id: request.request_id(),
                            response_deadline_monotonic_ms: None,
                            task_result: TaskOutcome::Success,
                            final_page: Some("neutral/done".to_owned()),
                            executed_steps: 1,
                        },
                    )
                    .after(&release)
                })
                .ledger(Vec::new()),
        ]
    });
    let expired = context.line(&[
        "--authorization",
        AUTHORIZATION,
        "--expect",
        DONE_KEY,
        "--wait",
        "0ms",
    ]);
    let details = refused(&expired, "command_wait_expired", 4);
    assert!(!context.trials().exists(), "a record was written");
    release.open();
    let record = runtime.finish();
    // The run, sent once; its receipt's projection is the client's own read once it came.
    assert_eq!(
        operations(&record.connections[0]),
        frames(&["status", "run_contained_task", "query_events"])
    );
    let request_id = sent_request_id(&record.connections[0]);
    assert_eq!(details["request_id"], request_id.as_str(), "{details}");
    assert_eq!(
        details["outcome"]["values"]["request_id"],
        request_id.as_str(),
        "{details}"
    );
    let package = std::path::absolute(context.package())
        .unwrap()
        .display()
        .to_string();
    let state = context.state().display().to_string();
    let next = json!([
        "standard-pack",
        "trial",
        "--instance",
        ALIAS,
        "--package",
        package,
        "--authorization",
        AUTHORIZATION,
        "--expect",
        DONE_KEY,
        "--state-dir",
        state,
        "--collect",
        request_id
    ]);
    assert_eq!(details["next"], json!({"collect": next}), "{details}");

    // The Runtime's ledger of that run: `next`, twice.
    let ledger = run_ledger(
        instance,
        PACKAGE_ID,
        End::Keyed(DONE_KEY, "neutral/done"),
        Some((
            serde_json::from_value::<RequestId>(json!(request_id)).expect("a request id"),
            None,
        )),
    );
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .ledger(ledger.clone()),
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .ledger(ledger),
        ]
    });
    let collect = || {
        let words = next
            .as_array()
            .expect("the words")
            .iter()
            .map(|word| word.as_str().expect("a word").to_owned());
        run_cli(std::iter::once("--json".to_owned()).chain(words), true)
    };
    let first = collect();
    let second = collect();
    let record = runtime.finish();
    let reads = frames(&[
        "status",
        "query_events",
        "query_events",
        "query_events",
        "query_events",
    ]);
    assert_eq!(operations(&record.connections[0]), reads);
    assert_eq!(operations(&record.connections[1]), reads);
    assert!(record.unscripted.is_empty());

    let shown = first.envelope_json();
    assert_eq!(first.exit_code(), 0, "{shown}");
    let data = first.envelope.data.clone().expect("the answer");
    assert_eq!(data["sent"], false, "{shown}");
    assert_eq!(data["request_id"], request_id.as_str(), "{shown}");
    assert_eq!(
        data["end"],
        json!({"state": "succeeded", "outcome_key": DONE_KEY, "final_page": "neutral/done"}),
        "{shown}"
    );
    assert_eq!(data["matched"], true, "{shown}");
    let name = record_name(&data["plan"]);
    let place = context.trials().join(&name);
    assert_eq!(
        data["record"],
        place.display().to_string().as_str(),
        "{shown}"
    );
    let kept = record_at(&data["record"]);
    assert_eq!(kept["request_id"], request_id.as_str());
    assert_eq!(kept["plan"]["authorization"], AUTHORIZATION);

    let details = refused(&second, "trial_record_exists", 2);
    assert_eq!(
        details["outcome"]["values"],
        json!({"path": place.display().to_string()}),
        "{details}"
    );
    assert_eq!(details["request_id"], request_id.as_str(), "{details}");
    let names = fs::read_dir(context.trials())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec![name]);
}

#[test]
fn s311_collect_of_another_packages_run_is_refused() {
    let context = Context::new();
    let instance = IdentifierIssuer::new()
        .expect("identifier issuer")
        .mint_instance_id()
        .expect("instance id");
    let instance_id = *instance.transport();
    let ledger = run_ledger(
        instance,
        OTHER_PACKAGE,
        End::Keyed(DONE_KEY, "neutral/done"),
        None,
    );
    let request_id = request_id_of(&ledger);
    let runtime = ScriptedRuntime::start(&context.runtime_root(), |owner_epoch| {
        vec![
            ScriptedConnection::new()
                .request(status(owner_epoch, instance_id))
                .ledger(ledger),
        ]
    });
    let answered = context.line(&[
        "--authorization",
        AUTHORIZATION,
        "--collect",
        request_id.as_str(),
    ]);
    let record = runtime.finish();
    assert_eq!(
        operations(&record.connections[0]),
        frames(&["status", "query_events", "query_events", "query_events"])
    );
    let details = refused(&answered, "trial_collect_mismatch", 2);
    assert_eq!(
        details["outcome"]["values"],
        json!({"request_id": request_id, "trial_collect_problem": "other_package"}),
        "{details}"
    );
    assert_eq!(details["sent"], false, "{details}");
    assert!(!context.trials().exists(), "a record was written");
}
