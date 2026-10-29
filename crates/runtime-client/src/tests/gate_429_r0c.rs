// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-0c (register §1.216; contracts/runtime-interface.md, "Workbench request
// kinds"): the runtime client's methods for the workbench's request kinds and members, each over
// `ScriptedRuntime`, with no sleep and no device.
// 1. Each method sends its operation member by member: `run_instance_task` mints its holder on
//    the connection and always names its deadline; `expected_version` travels as given and is
//    absent from the wire when `None`.
// 2. Each method reads its completed answer; `config_version` is passed on, `Some` or `None`, by
//    `configure_settings` and `runtime_fact_snapshot_versioned`, and `runtime_fact_snapshot`
//    answers the snapshot as before.
// 3. The 0.12.30 Runtime's Denied `not_implemented` is the method's error with the host's code
//    and operation, never an unexpected result, and the request is sent once.
// 4. `run_instance_task` after a cancelled run or a receipt timeout sends neither `SafeReset` nor
//    `CancelContainedTask`.
// 5. `stop_instance_run` refuses an answer for another run and a `reset` that contradicts
//    `settled`.
// 6. `routine_summary` (and `pack_schedule_view`, the same way) reports an older Runtime that
//    drops the connection as `runtime_operation_unsupported` and refuses another instance's rows.
// 7. Each method refuses a result of another kind as unexpected.

use crate::test_support::{ScriptedAnswer, ScriptedConnection, ScriptedRecord, ScriptedRuntime};
use crate::{
    ClientCode, ClientLocation, ConfiguredSettings, RuntimeClient, RuntimeClientConfig,
    RuntimeClientError, RuntimeClientErrorClass, RuntimeClientResult,
};
use actingcommand_contract::codes::{RuntimeContractCode, SharedCode};
use actingcommand_contract::outcome::{Code, Detail, Location, Outcome, OutcomeEnvelope};
use actingcommand_contract::{
    ConfigParameterSource, ConfigSettingChange, ConfiguredSetting, ContainedTaskCancellationReason,
    ContainedTaskRequest, EventActor, EventSource, FactScalar, FactScope, IdentifierIssuer,
    InstanceRunReset, InstanceRunStop, OwnerEpoch, PackScheduleEditRow, PackScheduleEdited,
    PackSchedulePackage, PackScheduleScopeVersion, PackScheduleSource, PackScheduleView,
    RUNTIME_FACT_SCHEMA_VERSION, RUNTIME_RECEIPT_SCHEMA_VERSION, RequestId, ResourceTargetEdit,
    ResourceTargetsApplied, RoutineSummaryGranularity, RoutineSummaryInstance, RoutineSummaryPage,
    RuntimeErrorCode, RuntimeErrorProjection, RuntimeFactSnapshot, RuntimeOperation,
    RuntimeOperationKind, RuntimeReceipt, RuntimeReceiptState, RuntimeRequest, RuntimeResult,
    TaskOutcome, TerminalEvent,
};
use serde_json::{Value, json};
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

/// An IO budget no answered exchange of these tests comes near.
const IO: Duration = Duration::from_secs(10);
/// An IO budget a receipt the script holds back outlasts.
const SHORT_IO: Duration = Duration::from_millis(250);
const INSTANCE: &str = "node-01";
const OTHER_INSTANCE: &str = "node-02";
const PACKAGE: &str = "neutral.package";

fn connect(state_root: &Path, io_timeout: Duration) -> RuntimeClient {
    connect_as(state_root, io_timeout, EventActor::Cli, EventSource::Cli)
}

/// `ApplyResourceTargets` keeps its Agent/Adapter origin gate until U-6.
fn connect_agent(state_root: &Path) -> RuntimeClient {
    connect_as(state_root, IO, EventActor::Agent, EventSource::Adapter)
}

fn connect_as(
    state_root: &Path,
    io_timeout: Duration,
    actor: EventActor,
    source: EventSource,
) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(state_root, crate::RuntimeChannel::Cli, actor, source)
            .with_io_timeout(io_timeout),
    )
    .expect("runtime client")
}

fn ids() -> IdentifierIssuer {
    IdentifierIssuer::new().expect("identifier issuer")
}

/// A completed receipt of `request` carrying `result`. The workbench's kinds get their success
/// codes with their behaviour (register §1.213), so the script tops a completed receipt of one
/// with `Status`' success code: the client reads a completed receipt by its state and result.
fn completed(
    request: &RuntimeRequest,
    terminal: Option<TerminalEvent>,
    result: RuntimeResult,
) -> ScriptedAnswer {
    let code = request
        .operation()
        .kind()
        .success_code()
        .unwrap_or_else(|| {
            RuntimeOperationKind::Status
                .success_code()
                .expect("Status completes")
        });
    let mut receipt = json!({
        "schema_version": RUNTIME_RECEIPT_SCHEMA_VERSION,
        "request_id": request.request_id(),
        "correlation_id": request.correlation_id(),
        "state": "completed",
        "result": result,
        "outcome": OutcomeEnvelope::new(&Outcome::new(code), Detail::Full),
    });
    if let Some(terminal) = terminal {
        receipt["terminal"] = json!(terminal);
    }
    let receipt: RuntimeReceipt =
        serde_json::from_value(receipt).expect("a completed scripted receipt");
    receipt.validate().expect("the completed receipt validates");
    ScriptedAnswer::Receipt(Box::new(receipt))
}

/// Runtime 0.12.30's answer to a kind or a member without behaviour (runtime-host
/// `not_implemented`): Denied, `not_implemented` on top naming `operation`, the Runtime error
/// code `invalid_request`, not halted.
fn not_implemented(request: &RuntimeRequest, operation: &str) -> ScriptedAnswer {
    let outcome = RuntimeErrorProjection::new(RuntimeErrorCode::InvalidRequest, false)
        .receipt_outcome(Some(SharedCode::NotImplemented.code()), None);
    let receipt = RuntimeReceipt::failure(request, RuntimeReceiptState::Denied, None, outcome)
        .expect("a denied scripted receipt");
    let mut encoded = serde_json::to_value(&receipt).expect("encode the receipt");
    encoded["outcome"]["values"]["operation"] = json!(operation);
    let receipt: RuntimeReceipt = serde_json::from_value(encoded).expect("decode the receipt");
    receipt.validate().expect("the denied receipt validates");
    ScriptedAnswer::Receipt(Box::new(receipt))
}

fn contained_task_completed(request: &RuntimeRequest) -> RuntimeResult {
    let ids = ids();
    RuntimeResult::ContainedTaskCompleted {
        run_id: *ids.mint_run_id().expect("run id").transport(),
        task_id: *ids.mint_task_id().expect("task id").transport(),
        task_request_id: request.request_id(),
        response_deadline_monotonic_ms: Some(60_000),
        task_result: TaskOutcome::Success,
        final_page: None,
        executed_steps: 1,
    }
}

fn contained_task_cancelled(
    request: &RuntimeRequest,
    reason: ContainedTaskCancellationReason,
) -> RuntimeResult {
    let ids = ids();
    RuntimeResult::ContainedTaskCancelled {
        run_id: *ids.mint_run_id().expect("run id").transport(),
        task_id: *ids.mint_task_id().expect("task id").transport(),
        task_request_id: request.request_id(),
        response_deadline_monotonic_ms: Some(60_000),
        reason,
        lease_terminal: None,
    }
}

fn request_id() -> RequestId {
    *ids().mint_request_id().expect("request id").transport()
}

fn terminal(sequence: u64) -> TerminalEvent {
    TerminalEvent {
        sequence,
        event_id: *ids().mint_event_id().expect("event id").transport(),
    }
}

fn changes() -> Vec<ConfigSettingChange> {
    vec![ConfigSettingChange {
        key: "scheduling.parallel.enabled".to_owned(),
        value: Some(json!(true)),
    }]
}

fn configured_setting() -> ConfiguredSetting {
    ConfiguredSetting {
        key: "scheduling.parallel.enabled".to_owned(),
        value: FactScalar::Boolean(true),
        source: ConfigParameterSource::Instruction,
    }
}

fn fact_snapshot() -> RuntimeFactSnapshot {
    RuntimeFactSnapshot {
        schema_version: RUNTIME_FACT_SCHEMA_VERSION.to_owned(),
        ledger_position: 9,
        taken_at_unix_ms: 1_000,
        snapshot_id: 9,
        part: 1,
        parts: 1,
        records: Vec::new(),
    }
}

/// An applied table of `instance_alias` at `terminal`, which its receipt names.
fn applied(instance_alias: &str, terminal: TerminalEvent) -> ResourceTargetsApplied {
    ResourceTargetsApplied {
        instance_alias: instance_alias.to_owned(),
        policy_sha256: format!("sha256:{}", "a".repeat(64)),
        version: terminal.sequence,
        event_id: terminal.event_id,
        previous_version: None,
        replayed: false,
        checked_catalog_hash: format!("sha256:{}", "b".repeat(64)),
        valid_until_unix_ms: None,
        conditions_at_position: terminal.sequence,
        conditions: Vec::new(),
    }
}

fn rows() -> Vec<PackScheduleEditRow> {
    vec![PackScheduleEditRow {
        from_unix_ms: 1_000,
        until_unix_ms: None,
        schedulable: false,
    }]
}

fn summary(owner_epoch: OwnerEpoch, aliases: &[&str]) -> RoutineSummaryPage {
    RoutineSummaryPage {
        owner_epoch,
        granularity: RoutineSummaryGranularity::Hour,
        revision: 4,
        instances: aliases
            .iter()
            .map(|alias| RoutineSummaryInstance {
                instance_alias: (*alias).to_owned(),
                coverage_from_unix_ms: 1_000,
                buckets: Vec::new(),
            })
            .collect(),
    }
}

fn pack_schedule(instance_alias: &str) -> PackScheduleView {
    PackScheduleView {
        instance_alias: instance_alias.to_owned(),
        scopes: vec![PackScheduleScopeVersion {
            scope: FactScope::Instance {
                instance_id: instance_alias.to_owned(),
            },
            version: 0,
        }],
        packages: vec![PackSchedulePackage {
            package_id: PACKAGE.to_owned(),
            schedulable: true,
            source: PackScheduleSource::None,
            row: None,
            next_change_unix_ms: None,
        }],
    }
}

/// Every request of `kind` one connection received, as it came on the wire, in order.
fn sent(record: &ScriptedRecord, connection: usize, kind: RuntimeOperationKind) -> Vec<Value> {
    record.connections[connection]
        .requests
        .iter()
        .filter(|request| request.operation().kind() == kind)
        .map(|request| serde_json::to_value(request.operation()).expect("encode the operation"))
        .collect()
}

fn assert_code(error: &RuntimeClientError, code: Code, row: &str) {
    assert_eq!(error.code().as_str(), code.as_str(), "{row}: {error:?}");
}

fn assert_unexpected(error: &RuntimeClientError, operation: Location, row: &str) {
    assert_code(error, ClientCode::RuntimeResultUnexpected.code(), row);
    assert_eq!(error.operation().as_str(), operation.as_str(), "{row}");
}

// 1 and 2: `run_instance_task` mints a holder on its connection for each call, names the deadline
// it is given in milliseconds, refuses a deadline outside 1 ms to the contained task's maximum
// before sending anything, and answers a completed run's flow.
#[test]
fn r0c_run_instance_task_mints_its_holder_names_its_deadline_and_answers_its_flow() {
    let root = TempDir::new().expect("tempdir");
    let answer = |request: &RuntimeRequest| {
        assert_eq!(
            request.operation().kind(),
            RuntimeOperationKind::RunInstanceTask
        );
        completed(request, None, contained_task_completed(request))
    };
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![
            // The flow's projection query follows each run.
            ScriptedConnection::new().request(answer).ledger(Vec::new()),
            ScriptedConnection::new().request(answer).ledger(Vec::new()),
        ]
    });
    let first = connect(root.path(), IO);
    for deadline in [
        Duration::ZERO,
        Duration::from_micros(999),
        Duration::from_millis(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS + 1),
    ] {
        let refused = first
            .run_instance_task(INSTANCE, PACKAGE, deadline)
            .expect_err("a deadline out of range");
        assert_code(
            &refused,
            RuntimeContractCode::InvalidContainedTaskResponseDeadline.code(),
            "a deadline out of range",
        );
        assert_eq!(
            refused.operation().as_str(),
            ClientLocation::RuntimeRunInstanceTask.location().as_str()
        );
    }
    let output = first
        .run_instance_task(INSTANCE, PACKAGE, Duration::from_millis(30_000))
        .expect("the first run");
    assert!(matches!(
        output.receipt().result(),
        Some(RuntimeResult::ContainedTaskCompleted { .. })
    ));
    drop(first);
    let second = connect(root.path(), IO);
    second
        .run_instance_task(
            INSTANCE,
            PACKAGE,
            Duration::from_millis(ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS),
        )
        .expect("the second run");
    drop(second);

    let record = runtime.finish();
    let first = sent(&record, 0, RuntimeOperationKind::RunInstanceTask);
    let second = sent(&record, 1, RuntimeOperationKind::RunInstanceTask);
    assert_eq!(first.len(), 1, "the refused deadlines sent nothing");
    assert_eq!(second.len(), 1);
    for (operation, deadline) in [
        (&first[0], 30_000),
        (&second[0], ContainedTaskRequest::MAX_RESPONSE_DEADLINE_MS),
    ] {
        assert_eq!(operation["operation"], json!("run_instance_task"));
        assert_eq!(operation["instance_alias"], json!(INSTANCE));
        assert_eq!(operation["package_id"], json!(PACKAGE));
        assert_eq!(operation["response_deadline_ms"], json!(deadline));
        assert!(operation["holder_id"].is_string(), "{operation}");
    }
    assert_ne!(
        first[0]["holder_id"], second[0]["holder_id"],
        "each call mints its own holder"
    );
}

// 1 and 2: every other workbench method sends its operation member by member (`expected_version`
// as given, absent from the wire when `None`) and reads its completed answer; `config_version` is
// passed on whether present or not, and `runtime_fact_snapshot` answers the snapshot as before.
#[test]
fn r0c_each_method_sends_its_request_member_by_member_and_reads_its_completed_answer() {
    let root = TempDir::new().expect("tempdir");
    let stopped_run = request_id();
    let settled = InstanceRunStop {
        request_id: stopped_run,
        settled: true,
        reset: Some(InstanceRunReset::Done),
    };
    let stopping = InstanceRunStop {
        request_id: stopped_run,
        settled: false,
        reset: None,
    };
    let edited = PackScheduleEdited {
        version: 8,
        event_id: *ids().mint_event_id().expect("event id").transport(),
    };
    let edit_terminal = terminal(21);
    let apply_terminal = terminal(22);
    let settings_terminal = terminal(23);
    let runtime = ScriptedRuntime::start(root.path(), move |owner_epoch| {
        let script = ScriptedConnection::new()
            .request(move |request| {
                completed(
                    request,
                    None,
                    RuntimeResult::InstanceRunStopped { stopped: settled },
                )
            })
            .request(move |request| {
                completed(
                    request,
                    None,
                    RuntimeResult::InstanceRunStopped { stopped: stopping },
                )
            })
            .request(move |request| {
                completed(request, None, RuntimeResult::PackScheduleEdited { edited })
            })
            .request(move |request| {
                completed(request, None, RuntimeResult::PackScheduleEdited { edited })
            })
            .request(move |request| {
                completed(
                    request,
                    Some(edit_terminal),
                    RuntimeResult::ResourceTargetsApplied {
                        applied: Box::new(applied(INSTANCE, edit_terminal)),
                    },
                )
            })
            .request(move |request| {
                completed(
                    request,
                    None,
                    RuntimeResult::RoutineSummary {
                        summary: Box::new(summary(owner_epoch, &[INSTANCE])),
                    },
                )
            })
            .request(move |request| {
                completed(
                    request,
                    None,
                    RuntimeResult::RoutineSummary {
                        summary: Box::new(summary(owner_epoch, &[INSTANCE, OTHER_INSTANCE])),
                    },
                )
            })
            .request(|request| {
                completed(
                    request,
                    None,
                    RuntimeResult::PackScheduleViewed {
                        view: Box::new(pack_schedule(INSTANCE)),
                    },
                )
            })
            .request(move |request| {
                completed(
                    request,
                    Some(settings_terminal),
                    RuntimeResult::SettingsConfigured {
                        settings: vec![configured_setting()],
                        config_version: Some(13),
                    },
                )
            })
            .request(move |request| {
                completed(
                    request,
                    Some(settings_terminal),
                    RuntimeResult::SettingsConfigured {
                        settings: vec![configured_setting()],
                        config_version: None,
                    },
                )
            })
            .request(|request| {
                completed(
                    request,
                    None,
                    RuntimeResult::RuntimeFactSnapshot {
                        snapshot: fact_snapshot(),
                        config_version: Some(13),
                    },
                )
            })
            .request(|request| {
                completed(
                    request,
                    None,
                    RuntimeResult::RuntimeFactSnapshot {
                        snapshot: fact_snapshot(),
                        config_version: None,
                    },
                )
            })
            .request(|request| {
                completed(
                    request,
                    None,
                    RuntimeResult::RuntimeFactSnapshot {
                        snapshot: fact_snapshot(),
                        config_version: Some(13),
                    },
                )
            });
        let agent = ScriptedConnection::new()
            .request(move |request| {
                completed(
                    request,
                    Some(apply_terminal),
                    RuntimeResult::ResourceTargetsApplied {
                        applied: Box::new(applied(INSTANCE, apply_terminal)),
                    },
                )
            })
            .request(move |request| {
                completed(
                    request,
                    Some(apply_terminal),
                    RuntimeResult::ResourceTargetsApplied {
                        applied: Box::new(applied(INSTANCE, apply_terminal)),
                    },
                )
            });
        vec![script, agent]
    });
    let owner_epoch = runtime.owner_epoch();

    let client = connect(root.path(), IO);
    assert_eq!(
        client
            .stop_instance_run(INSTANCE, stopped_run, Duration::from_millis(25_000))
            .expect("a settled stop"),
        settled
    );
    assert_eq!(
        client
            .stop_instance_run(INSTANCE, stopped_run, Duration::from_millis(1_000))
            .expect("a stop still stopping"),
        stopping
    );
    let scope = FactScope::Instance {
        instance_id: INSTANCE.to_owned(),
    };
    assert_eq!(
        client
            .edit_pack_schedule(scope.clone(), PACKAGE, rows(), Some(7))
            .expect("an edit at a version"),
        edited
    );
    assert_eq!(
        client
            .edit_pack_schedule(scope.clone(), PACKAGE, rows(), None)
            .expect("an edit without a version"),
        edited
    );
    let pause = ResourceTargetEdit::SetPaused {
        id: "target.a".to_owned(),
        paused: true,
    };
    assert_eq!(
        client
            .edit_resource_target(INSTANCE, 0, pause.clone())
            .expect("a target edit"),
        applied(INSTANCE, edit_terminal)
    );
    assert_eq!(
        client
            .routine_summary(Some(INSTANCE), RoutineSummaryGranularity::Hour, Some(3))
            .expect("one instance's summary"),
        summary(owner_epoch, &[INSTANCE])
    );
    assert_eq!(
        client
            .routine_summary(None, RoutineSummaryGranularity::Day, None)
            .expect("every instance's summary"),
        summary(owner_epoch, &[INSTANCE, OTHER_INSTANCE])
    );
    assert_eq!(
        client
            .pack_schedule_view(INSTANCE)
            .expect("the pack schedule"),
        pack_schedule(INSTANCE)
    );
    assert_eq!(
        client
            .configure_settings(changes(), Some(12))
            .expect("a change at a version"),
        ConfiguredSettings {
            settings: vec![configured_setting()],
            config_version: Some(13),
        }
    );
    assert_eq!(
        client
            .configure_settings(changes(), None)
            .expect("a change without a version"),
        ConfiguredSettings {
            settings: vec![configured_setting()],
            config_version: None,
        }
    );
    assert_eq!(
        client
            .runtime_fact_snapshot_versioned()
            .expect("a versioned snapshot"),
        (fact_snapshot(), Some(13))
    );
    assert_eq!(
        client
            .runtime_fact_snapshot_versioned()
            .expect("a snapshot without a version"),
        (fact_snapshot(), None)
    );
    assert_eq!(
        client.runtime_fact_snapshot().expect("the snapshot"),
        fact_snapshot()
    );
    drop(client);
    let agent = connect_agent(root.path());
    assert_eq!(
        agent
            .apply_resource_targets("{}".to_owned(), Some(7))
            .expect("an apply at a version"),
        applied(INSTANCE, apply_terminal)
    );
    assert_eq!(
        agent
            .apply_resource_targets("{}".to_owned(), None)
            .expect("an apply without a version"),
        applied(INSTANCE, apply_terminal)
    );
    drop(agent);

    let record = runtime.finish();
    let stops = sent(&record, 0, RuntimeOperationKind::StopInstanceRun);
    assert_eq!(
        stops,
        [25_000, 1_000].map(|wait_ms| json!({
            "operation": "stop_instance_run",
            "instance_alias": INSTANCE,
            "request_id": stopped_run,
            "wait_ms": wait_ms,
        }))
    );
    let row = json!([{"from_unix_ms": 1_000, "schedulable": false}]);
    let scope = json!({"kind": "instance", "instance_id": INSTANCE});
    assert_eq!(
        sent(&record, 0, RuntimeOperationKind::EditPackSchedule),
        [
            json!({
                "operation": "edit_pack_schedule",
                "scope": scope,
                "package_id": PACKAGE,
                "rows": row,
                "expected_version": 7,
            }),
            json!({
                "operation": "edit_pack_schedule",
                "scope": scope,
                "package_id": PACKAGE,
                "rows": row,
            }),
        ]
    );
    assert_eq!(
        sent(&record, 0, RuntimeOperationKind::EditResourceTarget),
        [json!({
            "operation": "edit_resource_target",
            "instance_alias": INSTANCE,
            "expected_version": 0,
            "op": {"kind": "set_paused", "id": "target.a", "paused": true},
        })]
    );
    assert_eq!(
        sent(&record, 0, RuntimeOperationKind::RoutineSummary),
        [
            json!({
                "operation": "routine_summary",
                "instance": INSTANCE,
                "granularity": "hour",
                "since_revision": 3,
            }),
            json!({"operation": "routine_summary", "granularity": "day"}),
        ]
    );
    assert_eq!(
        sent(&record, 0, RuntimeOperationKind::PackScheduleView),
        [json!({"operation": "pack_schedule_view", "instance_alias": INSTANCE})]
    );
    let change = json!([{"key": "scheduling.parallel.enabled", "value": true}]);
    assert_eq!(
        sent(&record, 0, RuntimeOperationKind::ConfigureSettings),
        [
            json!({"operation": "configure_settings", "changes": change, "expected_version": 12}),
            json!({"operation": "configure_settings", "changes": change}),
        ]
    );
    assert_eq!(
        sent(&record, 0, RuntimeOperationKind::RuntimeFactSnapshot).len(),
        3
    );
    assert_eq!(
        sent(&record, 1, RuntimeOperationKind::ApplyResourceTargets),
        [
            json!({"operation": "apply_resource_targets", "document_json": "{}", "expected_version": 7}),
            json!({"operation": "apply_resource_targets", "document_json": "{}"}),
        ]
    );
}

// One method's call and the host operation that Runtime 0.12.30 names when it refuses it.
type Call = (
    &'static str,
    RuntimeOperationKind,
    fn(&RuntimeClient) -> RuntimeClientResult<()>,
);

fn run_instance_task(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client
        .run_instance_task(INSTANCE, PACKAGE, Duration::from_millis(30_000))
        .map(drop)
}

fn stop_instance_run(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client
        .stop_instance_run(INSTANCE, request_id(), Duration::from_millis(1_000))
        .map(drop)
}

fn edit_pack_schedule(client: &RuntimeClient) -> RuntimeClientResult<()> {
    let scope = FactScope::Instance {
        instance_id: INSTANCE.to_owned(),
    };
    client
        .edit_pack_schedule(scope, PACKAGE, rows(), None)
        .map(drop)
}

fn edit_resource_target(client: &RuntimeClient) -> RuntimeClientResult<()> {
    let delete = ResourceTargetEdit::Delete {
        id: "target.a".to_owned(),
    };
    client.edit_resource_target(INSTANCE, 0, delete).map(drop)
}

fn routine_summary(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client
        .routine_summary(Some(INSTANCE), RoutineSummaryGranularity::Hour, None)
        .map(drop)
}

fn pack_schedule_view(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client.pack_schedule_view(INSTANCE).map(drop)
}

fn configure_settings(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client.configure_settings(changes(), Some(12)).map(drop)
}

fn runtime_fact_snapshot_versioned(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client.runtime_fact_snapshot_versioned().map(drop)
}

fn apply_resource_targets(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client
        .apply_resource_targets("{}".to_owned(), None)
        .map(drop)
}

fn calls() -> [Call; 8] {
    [
        (
            "run_instance_task",
            RuntimeOperationKind::RunInstanceTask,
            run_instance_task,
        ),
        (
            "stop_instance_run",
            RuntimeOperationKind::StopInstanceRun,
            stop_instance_run,
        ),
        (
            "edit_pack_schedule",
            RuntimeOperationKind::EditPackSchedule,
            edit_pack_schedule,
        ),
        (
            "edit_resource_target",
            RuntimeOperationKind::EditResourceTarget,
            edit_resource_target,
        ),
        (
            "routine_summary",
            RuntimeOperationKind::RoutineSummary,
            routine_summary,
        ),
        (
            "pack_schedule_view",
            RuntimeOperationKind::PackScheduleView,
            pack_schedule_view,
        ),
        (
            "configure_settings",
            RuntimeOperationKind::ConfigureSettings,
            configure_settings,
        ),
        (
            "runtime_fact_snapshot",
            RuntimeOperationKind::RuntimeFactSnapshot,
            runtime_fact_snapshot_versioned,
        ),
    ]
}

// 3: Runtime 0.12.30 refuses each workbench kind, and a change or an apply that names
// `expected_version`, Denied `not_implemented` naming its operation. The method returns that
// refusal (`host_failure`), never an unexpected result, and the request is sent once. (A fact
// snapshot is answered in 0.12.30, so it is no row here; a refusal reads the same.)
#[test]
fn r0c_a_not_implemented_refusal_is_the_methods_error_and_is_sent_once() {
    let root = TempDir::new().expect("tempdir");
    let refused = calls()
        .into_iter()
        .filter(|(operation, _, _)| *operation != "runtime_fact_snapshot")
        .collect::<Vec<_>>();
    let runtime = ScriptedRuntime::start(root.path(), {
        let refused = refused
            .iter()
            .map(|(operation, kind, _)| (*operation, *kind))
            .collect::<Vec<_>>();
        move |_| {
            let mut script = ScriptedConnection::new();
            for (operation, kind) in refused {
                script = script.request(move |request| {
                    assert_eq!(request.operation().kind(), kind, "{operation}");
                    not_implemented(request, operation)
                });
            }
            let agent = ScriptedConnection::new().request(|request| {
                assert_eq!(
                    request.operation().kind(),
                    RuntimeOperationKind::ApplyResourceTargets
                );
                not_implemented(request, "apply_resource_targets")
            });
            vec![script, agent]
        }
    });
    let client = connect(root.path(), IO);
    let mut errors = refused
        .iter()
        .map(|(operation, _, call)| {
            (
                *operation,
                call(&client).expect_err("refused not_implemented"),
            )
        })
        .collect::<Vec<_>>();
    drop(client);
    let agent = connect_agent(root.path());
    errors.push((
        "apply_resource_targets",
        agent
            .apply_resource_targets("{}".to_owned(), Some(7))
            .expect_err("refused not_implemented"),
    ));
    drop(agent);

    for (operation, error) in &errors {
        assert_eq!(
            error.host_failure(),
            Some(("not_implemented", *operation)),
            "{operation}: {error:?}"
        );
        assert_code(error, ClientCode::RuntimeRequestRejected.code(), operation);
        assert!(!error.is_fatal(), "{operation}: {error:?}");
        assert_ne!(
            error.disposition(),
            RuntimeClientErrorClass::Uncertain,
            "{operation}: the refusal changed nothing"
        );
    }
    let record = runtime.finish();
    for (operation, kind, _) in &refused {
        assert_eq!(sent(&record, 0, *kind).len(), 1, "{operation}: sent once");
    }
    assert_eq!(
        sent(&record, 1, RuntimeOperationKind::ApplyResourceTargets).len(),
        1
    );
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
}

fn resets_and_cancels(record: &ScriptedRecord) -> usize {
    record
        .connections
        .iter()
        .chain(&record.unscripted)
        .map(|connection| {
            connection.count(|operation| {
                matches!(
                    operation,
                    RuntimeOperation::SafeReset { .. }
                        | RuntimeOperation::CancelContainedTask { .. }
                )
            })
        })
        .sum()
}

// 4: a cancelled direct run is the error its reason maps to, as for `run_contained_task`, and a
// receipt that never came is the receipt timeout. In neither case does the client send
// `SafeReset` or `CancelContainedTask`: the Runtime resets the device itself (#429 A.2 item 2),
// and the caller reads `Status`' `activity` and stops the run with `stop_instance_run`.
#[test]
fn r0c_run_instance_task_neither_resets_nor_cancels_after_a_cancelled_run_or_a_timeout() {
    let reasons = [
        (
            ContainedTaskCancellationReason::DeadlineExceeded,
            ClientCode::RuntimeContainedTaskResponseTimeout,
        ),
        (
            ContainedTaskCancellationReason::ClientRequested,
            ClientCode::RuntimeContainedTaskCancelled,
        ),
        (
            ContainedTaskCancellationReason::RecoveredAfterRestart,
            ClientCode::RuntimeContainedTaskCancelled,
        ),
        (
            ContainedTaskCancellationReason::BlackoutStop,
            ClientCode::RuntimeContainedTaskCancelled,
        ),
        (
            ContainedTaskCancellationReason::PerformanceYield,
            ClientCode::RuntimeContainedTaskCancelled,
        ),
        (
            ContainedTaskCancellationReason::PausedByOperator,
            ClientCode::RuntimeContainedTaskPaused,
        ),
    ];
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), move |_| {
        let mut script = ScriptedConnection::new();
        for (reason, _) in reasons {
            script = script.request(move |request| {
                ScriptedAnswer::answered(
                    request,
                    RuntimeReceiptState::Cancelled,
                    contained_task_cancelled(request, reason),
                )
            });
        }
        vec![script]
    });
    let client = connect(root.path(), IO);
    for (reason, code) in reasons {
        let row = format!("{reason:?}");
        let error = client
            .run_instance_task(INSTANCE, PACKAGE, Duration::from_millis(30_000))
            .expect_err("a cancelled run");
        assert_code(&error, code.code(), &row);
        assert_eq!(
            error.operation().as_str(),
            ClientLocation::RuntimeRunInstanceTask.location().as_str(),
            "{row}"
        );
    }
    drop(client);
    let record = runtime.finish();
    assert_eq!(resets_and_cancels(&record), 0);
    assert_eq!(
        record.connections[0]
            .count(|operation| { matches!(operation, RuntimeOperation::RunInstanceTask { .. }) }),
        reasons.len()
    );
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);

    // The receipt never comes: the wait is the 1 ms deadline plus the IO budget, then the
    // receipt timeout, uncertain, as it is; no reconnect, no cancel, no reset.
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            assert_eq!(
                request.operation().kind(),
                RuntimeOperationKind::RunInstanceTask
            );
            ScriptedAnswer::StallUntilClosed
        })]
    });
    let client = connect(root.path(), SHORT_IO);
    let error = client
        .run_instance_task(INSTANCE, PACKAGE, Duration::from_millis(1))
        .expect_err("a receipt that never came");
    drop(client);
    assert_code(
        &error,
        ClientCode::RuntimeReceiptTimeout.code(),
        "the receipt timeout",
    );
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
    let record = runtime.finish();
    assert_eq!(resets_and_cancels(&record), 0);
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
}

// 5: `stop_instance_run` refuses as unexpected an answer that names another run, a still-stopping
// answer that carries a `reset`, and a settled answer without one.
#[test]
fn r0c_stop_instance_run_refuses_another_run_and_a_reset_that_contradicts_settled() {
    let stopped_run = request_id();
    let rows = [
        (
            "another run",
            InstanceRunStop {
                request_id: request_id(),
                settled: true,
                reset: Some(InstanceRunReset::NotNeeded),
            },
        ),
        (
            "still stopping with a reset",
            InstanceRunStop {
                request_id: stopped_run,
                settled: false,
                reset: Some(InstanceRunReset::Done),
            },
        ),
        (
            "settled without a reset",
            InstanceRunStop {
                request_id: stopped_run,
                settled: true,
                reset: None,
            },
        ),
    ];
    for (row, stopped) in rows {
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), move |_| {
            vec![ScriptedConnection::new().request(move |request| {
                completed(request, None, RuntimeResult::InstanceRunStopped { stopped })
            })]
        });
        let client = connect(root.path(), IO);
        let error = client
            .stop_instance_run(INSTANCE, stopped_run, Duration::from_millis(1_000))
            .expect_err(row);
        drop(client);
        assert_unexpected(
            &error,
            ClientLocation::RuntimeStopInstanceRun.location(),
            row,
        );
        runtime.finish();
    }
}

// 6: `routine_summary` and `pack_schedule_view` read through `send_or_dropped`. A Runtime that
// does not know the kind drops the connection after the request frame: once the same owner
// answers again, the call is `runtime_operation_unsupported`, which settles the request; an
// answer that names another instance is unexpected.
#[test]
fn r0c_the_two_reads_report_an_older_runtime_and_refuse_another_instance() {
    type Read = (
        &'static str,
        fn(&RuntimeClient) -> RuntimeClientResult<()>,
        Location,
    );
    let reads: [Read; 2] = [
        (
            "routine_summary",
            routine_summary,
            ClientLocation::RuntimeRoutineSummary.location(),
        ),
        (
            "pack_schedule_view",
            pack_schedule_view,
            ClientLocation::RuntimePackScheduleView.location(),
        ),
    ];
    for (row, read, location) in reads {
        // The older Runtime: the frame's connection ends without a receipt, the reconnect's
        // handshake answers the same owner epoch.
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), |_| {
            vec![
                ScriptedConnection::new().request(|_| ScriptedAnswer::Close),
                ScriptedConnection::new(),
            ]
        });
        let client = connect(root.path(), IO);
        let error = read(&client).expect_err("an older Runtime");
        drop(client);
        assert_code(&error, ClientCode::RuntimeOperationUnsupported.code(), row);
        assert_eq!(error.operation().as_str(), location.as_str(), "{row}");
        assert_ne!(
            error.disposition(),
            RuntimeClientErrorClass::Uncertain,
            "{row}: the request was not performed"
        );
        let record = runtime.finish();
        assert!(
            record.unscripted.is_empty(),
            "{row}: {:?}",
            record.unscripted
        );

        // Another instance's rows.
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), |owner_epoch| {
            vec![ScriptedConnection::new().request(move |request| {
                let result = match request.operation() {
                    RuntimeOperation::RoutineSummary { .. } => RuntimeResult::RoutineSummary {
                        summary: Box::new(summary(owner_epoch, &[INSTANCE, OTHER_INSTANCE])),
                    },
                    RuntimeOperation::PackScheduleView { .. } => {
                        RuntimeResult::PackScheduleViewed {
                            view: Box::new(pack_schedule(OTHER_INSTANCE)),
                        }
                    }
                    other => panic!("a workbench read, got {other:?}"),
                };
                completed(request, None, result)
            })]
        });
        let client = connect(root.path(), IO);
        let error = read(&client).expect_err("another instance");
        drop(client);
        assert_unexpected(&error, location, row);
        runtime.finish();
    }
}

// 7: each method refuses a completed answer of another kind as unexpected.
#[test]
fn r0c_each_method_refuses_a_result_of_another_kind() {
    let mut cases = calls()
        .into_iter()
        .map(|(operation, _, call)| (operation, call, false))
        .collect::<Vec<_>>();
    let apply: fn(&RuntimeClient) -> RuntimeClientResult<()> = apply_resource_targets;
    cases.push(("apply_resource_targets", apply, true));
    for (row, call, agent) in cases {
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), |owner_epoch| {
            vec![ScriptedConnection::new().request(move |request| {
                completed(request, None, RuntimeResult::Health { owner_epoch })
            })]
        });
        let client = if agent {
            connect_agent(root.path())
        } else {
            connect(root.path(), IO)
        };
        let error = call(&client).expect_err(row);
        drop(client);
        assert_code(&error, ClientCode::RuntimeResultUnexpected.code(), row);
        runtime.finish();
    }
}
