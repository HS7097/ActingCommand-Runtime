// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #455 RR4b-1 (0.12.104; register §1.557; contracts/runtime-interface.md, "Goal request
// kinds", "The goal page in pages"; `runtime-client` 15): `goal_page_view` over `ScriptedRuntime`,
// with no sleep and no device.
// 1. It sends `GoalPageView` member by member as given (`cursor` and `limit` absent from the wire
//    when `None`) through `send_or_dropped`, and answers the page the Runtime answered.
// 2. A Runtime that does not know the kind ends the connection after the request frame: once the
//    same owner answers again, the call is `runtime_operation_unsupported` (a client then shows the
//    goal page of revision 14); an answer that names another instance is unexpected.

use crate::test_support::{ScriptedAnswer, ScriptedConnection, ScriptedRuntime};
use crate::{
    ClientCode, ClientLocation, RuntimeClient, RuntimeClientConfig, RuntimeClientErrorClass,
};
use actingcommand_contract::outcome::{Detail, Outcome, OutcomeEnvelope};
use actingcommand_contract::{
    EventActor, EventSource, GoalDeclarationRead, GoalDeclarationState, GoalPageCursor,
    GoalPagePart, GoalPageRow, GoalPageTables, GoalPageView, GoalTablesRead,
    RUNTIME_RECEIPT_SCHEMA_VERSION, RuntimeOperation, RuntimeOperationKind, RuntimeReceipt,
    RuntimeRequest, RuntimeResult,
};
use serde_json::json;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

/// An IO budget no answered exchange of these tests comes near.
const IO: Duration = Duration::from_secs(10);
const INSTANCE: &str = "node-01";
const OTHER_INSTANCE: &str = "node-02";

fn connect(state_root: &Path) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(
            state_root,
            crate::RuntimeChannel::Cli,
            EventActor::Cli,
            EventSource::Cli,
        )
        .with_io_timeout(IO),
    )
    .expect("runtime client")
}

/// A completed receipt of `request` carrying `result`, its outcome the kind's success code.
fn completed(request: &RuntimeRequest, result: RuntimeResult) -> ScriptedAnswer {
    let code = request
        .operation()
        .kind()
        .success_code()
        .expect("GoalPageView completes with its own code");
    let receipt = json!({
        "schema_version": RUNTIME_RECEIPT_SCHEMA_VERSION,
        "request_id": request.request_id(),
        "correlation_id": request.correlation_id(),
        "state": "completed",
        "result": result,
        "outcome": OutcomeEnvelope::new(&Outcome::new(code), Detail::Full),
    });
    let receipt: RuntimeReceipt =
        serde_json::from_value(receipt).expect("a completed scripted receipt");
    receipt.validate().expect("the completed receipt validates");
    ScriptedAnswer::Receipt(Box::new(receipt))
}

/// A page of `instance_alias`'s part `options` with one class row, of three rows in all, and the
/// cursor of its next page.
fn page(instance_alias: &str) -> GoalPageView {
    GoalPageView {
        instance_alias: instance_alias.to_owned(),
        game_id: "neutral".to_owned(),
        version: 7,
        declaration: GoalDeclarationState {
            state: GoalDeclarationRead::Accepted,
            sha256: None,
            standard_pack_problem: None,
            json_pointer: None,
            path: None,
            io_kind: None,
        },
        tables: GoalPageTables {
            state: GoalTablesRead::Accepted,
            sha256: Some(format!("sha256:{}", "a".repeat(64))),
            standard_pack_problem: None,
            json_pointer: None,
        },
        read_at_unix_ms: 1_000,
        fresh_until_unix_ms: None,
        records_state: None,
        ladder: None,
        rows: vec![GoalPageRow::Class {
            class: "class_a".to_owned(),
            name: None,
        }],
        rows_total: 3,
        cut: Some(actingcommand_contract::GoalPageCut::Count),
        next: Some(cursor(2)),
    }
}

/// A cursor of the part `options` of the entry `raise` after `after` rows.
fn cursor(after: u32) -> GoalPageCursor {
    GoalPageCursor {
        records_version: None,
        tables_sha256: Some(format!("sha256:{}", "a".repeat(64))),
        entries_sha256: format!("sha256:{}", "b".repeat(64)),
        part_sha256: options().digest(),
        after,
    }
}

fn options() -> GoalPagePart {
    GoalPagePart::Options {
        entry: "raise".to_owned(),
    }
}

// 1: the request as given and the page as answered. The first call sends no cursor and no limit
// (absent from the wire); the second sends the first page's `next` and a limit of 2; each answers
// the scripted page whole.
#[test]
fn rr4b1_c1_goal_page_view_sends_its_members_as_given_and_answers_the_page() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![
            ScriptedConnection::new()
                .request(|request| {
                    completed(
                        request,
                        RuntimeResult::GoalPageViewed {
                            page: Box::new(page(INSTANCE)),
                        },
                    )
                })
                .request(|request| {
                    completed(
                        request,
                        RuntimeResult::GoalPageViewed {
                            page: Box::new(page(INSTANCE)),
                        },
                    )
                }),
        ]
    });
    let client = connect(root.path());
    let first = client
        .goal_page_view(INSTANCE, options(), None, None)
        .expect("the first page");
    assert_eq!(first, page(INSTANCE));
    let second = client
        .goal_page_view(INSTANCE, options(), first.next.clone(), Some(2))
        .expect("the next page");
    assert_eq!(second, page(INSTANCE));
    drop(client);
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);
    let sent = record.connections[0]
        .requests
        .iter()
        .filter(|request| request.operation().kind() == RuntimeOperationKind::GoalPageView)
        .map(|request| serde_json::to_value(request.operation()).expect("encode the operation"))
        .collect::<Vec<_>>();
    assert_eq!(
        sent,
        vec![
            json!({"operation": "goal_page_view", "instance_alias": INSTANCE,
                   "part": {"part": "options", "entry": "raise"}}),
            json!({"operation": "goal_page_view", "instance_alias": INSTANCE,
                   "part": {"part": "options", "entry": "raise"},
                   "cursor": serde_json::to_value(cursor(2)).expect("encode the cursor"),
                   "limit": 2}),
        ]
    );
}

// 2: a Runtime that does not know the kind (revision 14 or lower) ends the connection after the
// request frame; the reconnect's handshake answers the same owner epoch, so the call is
// `runtime_operation_unsupported`, settled (the request was not performed). A page that names
// another instance is unexpected.
#[test]
fn rr4b1_c2_an_older_runtime_is_unsupported_and_another_instances_page_unexpected() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![
            ScriptedConnection::new().request(|_| ScriptedAnswer::Close),
            ScriptedConnection::new(),
        ]
    });
    let client = connect(root.path());
    let error = client
        .goal_page_view(INSTANCE, GoalPagePart::Records, None, None)
        .expect_err("an older Runtime");
    drop(client);
    assert_eq!(
        error.code().as_str(),
        ClientCode::RuntimeOperationUnsupported.code().as_str(),
        "{error:?}"
    );
    assert_eq!(
        error.operation().as_str(),
        ClientLocation::RuntimeGoalPageView.location().as_str()
    );
    assert_ne!(error.disposition(), RuntimeClientErrorClass::Uncertain);
    let record = runtime.finish();
    assert!(record.unscripted.is_empty(), "{:?}", record.unscripted);

    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            let RuntimeOperation::GoalPageView { .. } = request.operation() else {
                panic!("a goal page view, not {:?}", request.operation());
            };
            completed(
                request,
                RuntimeResult::GoalPageViewed {
                    page: Box::new(page(OTHER_INSTANCE)),
                },
            )
        })]
    });
    let client = connect(root.path());
    let error = client
        .goal_page_view(INSTANCE, options(), None, None)
        .expect_err("another instance");
    drop(client);
    assert_eq!(
        error.code().as_str(),
        ClientCode::RuntimeResultUnexpected.code().as_str(),
        "{error:?}"
    );
    assert_eq!(
        error.operation().as_str(),
        ClientLocation::RuntimeGoalPageView.location().as_str()
    );
    runtime.finish();
}
