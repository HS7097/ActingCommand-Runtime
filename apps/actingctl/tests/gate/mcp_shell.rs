// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #382 MCP-T1 (test plan A-5a, inventory keys 28, 30, 31, 34 and 35): the MCP
//! server shell through the real `actingctl mcp-serve` over pipes (fixture I5, `mcp_harness`).
//! Rows without a Runtime give the server an empty state root; the others give it the root of
//! the c4 fake Runtime child (`tests/support/c4_runtime.rs`). Every server runs with the
//! default tier set and only observer tools are called. Every expected value comes from the
//! frozen #338 model and its rulings. JSON-RPC errors are read only through `rpc_error`, tool
//! errors only through `tool_error`.

use crate::c4_runtime::{RuntimeChild, write_sealed_frame};
use crate::mcp_harness::{McpServer, accepted};
use actingcommand_contract::{
    EventActor, EventQuery, EventSource, IdentifierIssuer, ProjectionProfile, RuntimeInfo,
    SchedulingPauseScope,
};
use actingcommand_runtime_client::{RuntimeClient, RuntimeClientConfig};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use tempfile::TempDir;

/// The stateless protocol version (#338 §四 双代协议).
const MODERN: &str = "2026-07-28";
/// Every version the server speaks, the modern one first.
const SUPPORTED: [&str; 4] = [MODERN, "2025-11-25", "2025-06-18", "2025-03-26"];
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;
/// A Lab job handle that no server process issued.
const UNHELD_LAB_JOB: &str = "lab_job_0000000000000000_1";
/// The structured payload budget of one tool result (#338 §四 预算).
const PAYLOAD_BUDGET: usize = 11 * 1024;
const CHILD_ENTRY: &str = "c4_runtime_child_process";
const ALIAS: &str = "neutral.shell";
const FILL_REASON: &str = "gate.fill";
const FILL_DRAIN_MS: u64 = 60_000;
/// Pause and resume pairs that put well over 100 rows into the ledger.
const LARGE_FILL: usize = 20;
/// Pause and resume pairs for a ledger of a few pages of 5.
const SMALL_FILL: usize = 2;
/// Far above any read here; only a cursor that never ends reaches it.
const MAX_PAGES: usize = 1_000;

#[test]
fn gate_initialize_answers_the_requested_legacy_version_and_2025_11_25_for_any_other() {
    for (requested, negotiated) in [
        ("2025-03-26", "2025-03-26"),
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2024-11-05", "2025-11-25"),
    ] {
        let (_root, mut mcp) = without_runtime();
        let answer = mcp.initialize_version(1, requested);
        let result = &answer["result"];
        assert_eq!(
            result["protocolVersion"],
            json!(negotiated),
            "initialize asking {requested}: {answer}"
        );
        assert_eq!(
            result["capabilities"]["tools"]["listChanged"],
            json!(false),
            "initialize asking {requested}: {answer}"
        );
        assert_eq!(
            result["serverInfo"]["name"],
            json!("actingctl-mcp"),
            "initialize asking {requested}: {answer}"
        );
    }
}

#[test]
fn gate_a_modern_request_is_served_without_initialize_and_every_modern_result_is_marked() {
    let (_root, mut mcp) = without_runtime();
    let discover = mcp.request(1, "server/discover", json!({"_meta": meta(MODERN, true)}));
    assert_eq!(
        discover["result"]["supportedVersions"],
        json!(SUPPORTED),
        "server/discover: {discover}"
    );
    assert!(
        discover["result"]["capabilities"]["tools"].is_object(),
        "server/discover: {discover}"
    );
    assert_modern_result(&discover, "server/discover");
    let listed = mcp.request(2, "tools/list", json!({"_meta": meta(MODERN, true)}));
    assert_modern_result(&listed, "modern tools/list");
    let called = mcp.request(
        3,
        "tools/call",
        json!({
            "name": "ac_get_run",
            "arguments": {"handle": UNHELD_LAB_JOB},
            "_meta": meta(MODERN, true),
        }),
    );
    assert_modern_result(&called, "modern tools/call");

    // Both eras in one process: a legacy session leaves modern requests modern.
    let initialized = mcp.initialize_version(4, "2025-11-25");
    assert!(
        initialized["result"].is_object(),
        "initialize: {initialized}"
    );
    let modern = mcp.request(5, "tools/list", json!({"_meta": meta(MODERN, true)}));
    assert_modern_result(&modern, "modern tools/list after initialize");
    let legacy = mcp.request(6, "tools/list", json!({}));
    assert!(
        legacy["result"].is_object()
            && legacy["result"].get("resultType").is_none()
            && legacy["result"].get("_meta").is_none(),
        "legacy tools/list after initialize: {legacy}"
    );
}

#[test]
fn gate_a_legacy_or_unknown_meta_version_is_32022_and_a_request_of_no_era_is_32602() {
    let (_root, mut mcp) = without_runtime();
    for (id, requested) in [(1, "2025-11-25"), (2, "2024-11-05")] {
        let answer = mcp.request(id, "tools/list", json!({"_meta": meta(requested, true)}));
        let (code, data) = rpc_error(&answer);
        assert_eq!(
            code, UNSUPPORTED_PROTOCOL_VERSION,
            "_meta version {requested}: {answer}"
        );
        assert_eq!(
            data["supported"],
            json!(SUPPORTED),
            "_meta version {requested}: {answer}"
        );
        assert_eq!(
            data["requested"],
            json!(requested),
            "_meta version {requested}: {answer}"
        );
    }
    let without_capabilities = mcp.request(3, "tools/list", json!({"_meta": meta(MODERN, false)}));
    assert_eq!(
        rpc_error(&without_capabilities).0,
        INVALID_PARAMS,
        "modern _meta without clientCapabilities: {without_capabilities}"
    );
    let without_era = mcp.request(4, "tools/list", json!({}));
    assert_eq!(
        rpc_error(&without_era).0,
        INVALID_PARAMS,
        "a request with neither initialize nor a _meta version: {without_era}"
    );
}

#[test]
fn gate_malformed_messages_unknown_methods_and_malformed_calls_get_their_json_rpc_codes() {
    let (_root, mut mcp) = without_runtime();
    mcp.send_line("this line is not JSON");
    let unparsed = mcp.recv();
    assert_eq!(
        rpc_error(&unparsed).0,
        PARSE_ERROR,
        "a line that is not JSON: {unparsed}"
    );
    assert_eq!(
        unparsed.get("id"),
        Some(&Value::Null),
        "a line that is not JSON: {unparsed}"
    );
    mcp.send(&json!({"jsonrpc": "1.0", "id": 2, "method": "ping"}));
    let invalid = mcp.recv();
    assert_eq!(
        rpc_error(&invalid).0,
        INVALID_REQUEST,
        "a message that is not JSON-RPC 2.0: {invalid}"
    );

    mcp.initialize_version(3, "2025-11-25");
    let unknown_method = mcp.request(4, "gate/no_such_method", json!({}));
    assert_eq!(
        rpc_error(&unknown_method).0,
        METHOD_NOT_FOUND,
        "an unknown method: {unknown_method}"
    );
    let unknown_tool = mcp.request(
        5,
        "tools/call",
        json!({"name": "ac_no_such_tool", "arguments": {}}),
    );
    assert_eq!(
        rpc_error(&unknown_tool).0,
        INVALID_PARAMS,
        "tools/call of an unknown tool: {unknown_tool}"
    );
    let arguments_not_object = mcp.request(
        6,
        "tools/call",
        json!({"name": "ac_get_run", "arguments": "not an object"}),
    );
    assert_eq!(
        rpc_error(&arguments_not_object).0,
        INVALID_PARAMS,
        "tools/call whose arguments are not an object: {arguments_not_object}"
    );
}

#[test]
fn gate_tool_definitions_and_call_results_take_the_shape_of_the_negotiated_version() {
    for version in ["2025-03-26", "2025-06-18", "2025-11-25"] {
        // 2025-03-26 knows neither title / outputSchema nor structuredContent.
        let structured = version != "2025-03-26";
        let (_root, mut mcp) = without_runtime();
        mcp.initialize_version(1, version);
        let listed = mcp.request(2, "tools/list", json!({}));
        let definitions = listed["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list under {version} lists no tools: {listed}"));
        assert!(
            !definitions.is_empty(),
            "tools/list under {version} lists no tools: {listed}"
        );
        for definition in definitions {
            for member in ["name", "description", "inputSchema", "annotations"] {
                assert!(
                    definition.get(member).is_some(),
                    "a tool definition under {version} without {member}: {definition}"
                );
            }
            for member in ["title", "outputSchema"] {
                assert_eq!(
                    definition.get(member).is_some(),
                    structured,
                    "member {member} of a tool definition under {version}: {definition}"
                );
            }
        }
        let called = mcp.call_tool(3, "ac_get_run", json!({"handle": UNHELD_LAB_JOB}));
        let members = called["result"]
            .as_object()
            .map(|result| result.keys().map(String::as_str).collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let expected: &[&str] = if structured {
            &["content", "isError", "structuredContent"]
        } else {
            &["content", "isError"]
        };
        assert_eq!(
            members,
            expected.iter().copied().collect::<BTreeSet<_>>(),
            "tools/call result under {version}: {called}"
        );
    }
}

#[test]
fn gate_a_batch_is_answered_as_one_array_under_2025_03_26_and_refused_after_later_versions() {
    let batch = json!([
        {"jsonrpc": "2.0", "id": 2, "method": "ping"},
        {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "ac_get_run", "arguments": {"handle": UNHELD_LAB_JOB}},
        },
    ]);
    {
        let (_root, mut mcp) = without_runtime();
        mcp.initialize_version(1, "2025-03-26");
        mcp.send(&batch);
        let answer = mcp.recv();
        let answers = answer
            .as_array()
            .unwrap_or_else(|| panic!("a 2025-03-26 batch is not answered by one array: {answer}"));
        assert_eq!(answers.len(), 2, "batch answer: {answer}");
        assert_eq!(
            answers
                .iter()
                .map(|answered| answered["id"].as_u64())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([Some(2), Some(3)]),
            "batch answer: {answer}"
        );
        assert!(
            answers
                .iter()
                .all(|answered| answered.get("result").is_some()),
            "batch answer: {answer}"
        );
    }
    for version in ["2025-06-18", "2025-11-25"] {
        let (_root, mut mcp) = without_runtime();
        mcp.initialize_version(1, version);
        mcp.send(&batch);
        let answer = mcp.recv();
        assert!(answer.is_object(), "a batch under {version}: {answer}");
        assert_eq!(
            rpc_error(&answer).0,
            INVALID_REQUEST,
            "a batch under {version}: {answer}"
        );
    }
}

#[test]
fn gate_a_legacy_ping_is_answered_before_initialize_and_a_modern_ping_is_32601() {
    let (_root, mut mcp) = without_runtime();
    let legacy = mcp.request(1, "ping", json!({}));
    assert_eq!(
        legacy["result"],
        json!({}),
        "a legacy ping before initialize: {legacy}"
    );
    let modern = mcp.request(2, "ping", json!({"_meta": meta(MODERN, true)}));
    assert_eq!(
        rpc_error(&modern).0,
        METHOD_NOT_FOUND,
        "a modern ping: {modern}"
    );
}

#[test]
fn gate_an_events_page_over_the_budget_is_truncated_and_its_cursor_resumes_after_its_last_row() {
    let (root, mut runtime, _info) = start_runtime();
    fill_ledger(root.path(), LARGE_FILL);
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let first = events_page(&mut mcp, &mut id, json!({"limit": 100}));
    let reference = every_page(&mut mcp, &mut id, 10)
        .into_iter()
        .flat_map(|page| page.rows)
        .collect::<Vec<_>>();
    // Precondition, a setup fault when it fails: the first 100 rows exceed the payload budget.
    assert!(
        reference.len() >= 100,
        "setup: the ledger holds {} rows, fewer than 100",
        reference.len()
    );
    let encoded = Value::Array(reference[..100].to_vec()).to_string().len();
    assert!(
        encoded > PAYLOAD_BUDGET,
        "setup: the first 100 rows encode to {encoded} bytes, not more than {PAYLOAD_BUDGET}"
    );

    assert!(
        first.rows.len() < 100,
        "a page of limit 100 over the budget holds {} rows",
        first.rows.len()
    );
    assert!(first.truncated, "a page over the budget is not truncated");
    assert!(
        first.next_cursor.is_some(),
        "a truncated page carries no next_cursor"
    );
    let expected = reference
        .iter()
        .map(seq)
        .filter(|sequence| *sequence <= first.snapshot)
        .collect::<Vec<_>>();
    let mut pages = vec![first];
    while let Some(cursor) = pages.last().and_then(|page| page.next_cursor.clone()) {
        assert!(
            pages.len() < MAX_PAGES,
            "ac_events still answers a next_cursor after {MAX_PAGES} pages"
        );
        pages.push(events_page(
            &mut mcp,
            &mut id,
            json!({"limit": 100, "cursor": cursor}),
        ));
    }
    let last_of_first = seq(pages[0].rows.last().expect("a truncated page holds rows"));
    let position = expected
        .iter()
        .position(|sequence| *sequence == last_of_first)
        .unwrap_or_else(|| panic!("row {last_of_first} is not in the first page's snapshot"));
    assert_eq!(
        pages[1].rows.first().map(seq),
        expected.get(position + 1).copied(),
        "the page after next_cursor does not start right after row {last_of_first}"
    );
    assert_eq!(
        sequences(&pages),
        expected,
        "paging to the end does not return every row of the first page's snapshot exactly once"
    );

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

#[test]
fn gate_get_run_answers_handle_unknown_for_job_handles_this_process_never_held() {
    let (_root, mut mcp) = without_runtime();
    mcp.initialize();
    let correlation = serde_json::to_value(
        *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_correlation_id()
            .expect("correlation id")
            .transport(),
    )
    .expect("correlation id JSON");
    for (id, handle) in [(2, json!(UNHELD_LAB_JOB)), (3, correlation)] {
        let answer = mcp.call_tool(id, "ac_get_run", json!({"handle": handle}));
        assert_eq!(
            tool_error(&answer),
            ("usage".to_owned(), "handle_unknown".to_owned()),
            "ac_get_run with handle {handle}: {answer}"
        );
    }
}

#[test]
fn gate_events_pages_follow_next_cursor_without_gap_or_repeat_up_to_the_first_snapshot() {
    let (root, mut runtime, _info) = start_runtime();
    fill_ledger(root.path(), SMALL_FILL);
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let pages = every_page(&mut mcp, &mut id, 5);
    assert!(
        pages.len() > 1,
        "setup: the ledger holds no more rows than one page of 5"
    );
    let snapshot = pages[0].snapshot;
    let expected = ledger_sequences(root.path())
        .into_iter()
        .filter(|sequence| *sequence <= snapshot)
        .collect::<Vec<_>>();
    assert_eq!(
        sequences(&pages),
        expected,
        "the pages do not match one read of the ledger up to the first page's snapshot {snapshot}"
    );

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

#[test]
fn gate_an_events_cursor_sent_with_other_filters_answers_cursor_invalid() {
    let (root, mut runtime, _info) = start_runtime();
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let cursor = first_cursor(&mut mcp, &mut id);
    let answer = mcp.call_tool(
        id + 1,
        "ac_events",
        json!({"limit": 1, "cursor": cursor, "view": "errors"}),
    );
    assert_eq!(
        tool_error(&answer),
        ("usage".to_owned(), "cursor_invalid".to_owned()),
        "a cursor sent with view errors: {answer}"
    );

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

#[test]
fn gate_an_events_cursor_from_before_a_runtime_restart_answers_cursor_invalid() {
    let (root, mut runtime, info) = start_runtime();
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    mcp.initialize();
    let mut id = 1;
    let cursor = first_cursor(&mut mcp, &mut id);
    // The Runtime stops cleanly and starts again on the same state root; the server keeps
    // running.
    runtime.restart(root.path(), &info);
    id += 1;
    let stale = mcp.call_tool(id, "ac_events", json!({"limit": 1, "cursor": cursor}));
    assert_eq!(
        tool_error(&stale),
        ("usage".to_owned(), "cursor_invalid".to_owned()),
        "a cursor issued before the Runtime restart: {stale}"
    );
    events_page(&mut mcp, &mut id, json!({"limit": 1}));

    mcp.close_input();
    runtime.stop_clean();
    drop(mcp);
}

#[test]
fn gate_an_events_cursor_of_another_server_process_answers_cursor_invalid() {
    let (root, mut runtime, _info) = start_runtime();
    let mut issuing = McpServer::spawn_with_tier(root.path(), None);
    issuing.initialize();
    let mut id = 1;
    let cursor = first_cursor(&mut issuing, &mut id);
    let mut other = McpServer::spawn_with_tier(root.path(), None);
    other.initialize();
    let answer = other.call_tool(2, "ac_events", json!({"limit": 1, "cursor": cursor}));
    assert_eq!(
        tool_error(&answer),
        ("usage".to_owned(), "cursor_invalid".to_owned()),
        "a cursor of another mcp-serve process: {answer}"
    );

    issuing.close_input();
    other.close_input();
    runtime.stop_clean();
    drop(issuing);
    drop(other);
}

#[test]
fn gate_at_stdin_eof_the_requests_already_read_are_answered_and_the_server_exits_0() {
    let (_root, mut mcp) = without_runtime();
    mcp.send(&initialize_request(1));
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    // Written right before stdin closes, without waiting for its answer.
    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "ac_get_run", "arguments": {"handle": UNHELD_LAB_JOB}},
    }));
    let (messages, status) = mcp.finish();
    for id in [1, 2] {
        assert!(
            messages
                .iter()
                .any(|message| message["id"] == json!(id) && message.get("result").is_some()),
            "request {id} read before stdin closed is not answered: {messages:?}"
        );
    }
    assert_eq!(status.code(), Some(0), "exit status at stdin EOF: {status}");
}

#[test]
fn gate_a_call_still_running_at_stdin_eof_does_not_hold_the_exit() {
    let (root, mut runtime, _info) = start_runtime();
    let mut mcp = McpServer::spawn_with_tier(root.path(), None);
    // A request id the Runtime never saw: the run stays not_found, which ac_get_run polls
    // for the whole wait.
    let unseen = serde_json::to_value(
        *IdentifierIssuer::new()
            .expect("identifier issuer")
            .mint_request_id()
            .expect("request id")
            .transport(),
    )
    .expect("request id JSON");
    mcp.send(&initialize_request(1));
    mcp.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    mcp.send(&json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {"name": "ac_get_run", "arguments": {"handle": unseen, "wait_s": 25}},
    }));
    let (messages, status) = mcp.finish();
    assert_eq!(status.code(), Some(0), "exit status at stdin EOF: {status}");
    assert!(
        !messages.iter().any(|message| message["id"] == json!(2)),
        "the call still running at stdin EOF was answered before stdout ended: {messages:?}"
    );

    runtime.stop_clean();
}

/// An `ac_events` page.
struct Page {
    rows: Vec<Value>,
    next_cursor: Option<String>,
    truncated: bool,
    snapshot: u64,
}

/// A JSON-RPC error answer's integer code and its `data` (null when it has none).
#[track_caller]
fn rpc_error(answer: &Value) -> (i64, &Value) {
    let error = answer
        .get("error")
        .unwrap_or_else(|| panic!("not a JSON-RPC error answer: {answer}"));
    let code = error["code"]
        .as_i64()
        .unwrap_or_else(|| panic!("a JSON-RPC error without an integer code: {answer}"));
    (code, &error["data"])
}

/// A tool error answer's class and code (`isError` true).
#[track_caller]
fn tool_error(answer: &Value) -> (String, String) {
    let result = &answer["result"];
    assert_eq!(result["isError"], json!(true), "not a tool error: {answer}");
    let error = &result["structuredContent"]["error"];
    let text = |member: &str| {
        error[member]
            .as_str()
            .unwrap_or_else(|| panic!("a tool error without {member}: {answer}"))
            .to_owned()
    };
    (text("class"), text("code"))
}

/// `params._meta` naming `version`, with an empty clientCapabilities object when `capabilities`.
fn meta(version: &str, capabilities: bool) -> Value {
    let mut meta = Map::new();
    meta.insert(
        "io.modelcontextprotocol/protocolVersion".to_owned(),
        json!(version),
    );
    if capabilities {
        meta.insert(
            "io.modelcontextprotocol/clientCapabilities".to_owned(),
            json!({}),
        );
    }
    Value::Object(meta)
}

/// A 2026-07-28 result: `resultType` complete and the server identity in `_meta`.
#[track_caller]
fn assert_modern_result(answer: &Value, what: &str) {
    let result = &answer["result"];
    assert_eq!(result["resultType"], json!("complete"), "{what}: {answer}");
    assert!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"].is_object(),
        "{what}: {answer}"
    );
}

fn initialize_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "gate-test-client", "version": "0.0.0"},
        },
    })
}

/// A server on a fresh empty state root: no Runtime answers there.
fn without_runtime() -> (TempDir, McpServer) {
    let root = TempDir::new().expect("tempdir");
    let mcp = McpServer::spawn_with_tier(root.path(), None);
    (root, mcp)
}

/// The c4 Runtime child on a fresh state root, ready.
fn start_runtime() -> (TempDir, RuntimeChild, RuntimeInfo) {
    let root = TempDir::new().expect("tempdir");
    write_sealed_frame(&root.path().join("sealed.png"));
    let mut runtime = RuntimeChild::spawn_for_instance(root.path(), CHILD_ENTRY, ALIAS);
    let info = runtime.wait_ready(root.path());
    (root, runtime, info)
}

/// Ordinary client writes, as actingctl's pause and resume make them: `pairs` instance pauses,
/// each lifted again, on a test-side (Cli, Cli) connection. Their rows carry the instance and
/// the request.
fn fill_ledger(root: &Path, pairs: usize) {
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a client to fill the ledger");
    let scope = SchedulingPauseScope::Instance {
        instance_alias: ALIAS.to_owned(),
    };
    for _ in 0..pairs {
        client
            .pause_scheduling(scope.clone(), FILL_REASON, FILL_DRAIN_MS)
            .expect("pause the instance");
        client
            .resume_scheduling(scope.clone())
            .expect("resume the instance");
    }
}

/// The sequence of every ledger event, in one read on a test-side connection.
fn ledger_sequences(root: &Path) -> Vec<u64> {
    let client = RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect a ledger client");
    client
        .query_events(EventQuery::default(), ProjectionProfile::Forensic)
        .expect("read the ledger")
        .iter()
        .map(|event| event.sequence)
        .collect()
}

/// One accepted `ac_events` page.
fn events_page(mcp: &mut McpServer, id: &mut u64, arguments: Value) -> Page {
    *id += 1;
    let answer = mcp.call_tool(*id, "ac_events", arguments);
    let result = accepted(&answer, "ac_events");
    Page {
        rows: result["events"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("an ac_events answer without events: {answer}")),
        next_cursor: result
            .get("next_cursor")
            .and_then(Value::as_str)
            .map(str::to_owned),
        truncated: result.get("truncated") == Some(&json!(true)),
        snapshot: result["snapshot_ledger_position"]
            .as_u64()
            .unwrap_or_else(|| {
                panic!("an ac_events answer without snapshot_ledger_position: {answer}")
            }),
    }
}

/// Every page of the whole ledger in pages of `limit`, each next page asked with the previous
/// page's next_cursor.
fn every_page(mcp: &mut McpServer, id: &mut u64, limit: u64) -> Vec<Page> {
    let mut pages = vec![events_page(mcp, id, json!({"limit": limit}))];
    while let Some(cursor) = pages.last().and_then(|page| page.next_cursor.clone()) {
        assert!(
            pages.len() < MAX_PAGES,
            "ac_events still answers a next_cursor after {MAX_PAGES} pages"
        );
        pages.push(events_page(
            mcp,
            id,
            json!({"limit": limit, "cursor": cursor}),
        ));
    }
    pages
}

/// The next_cursor of a first page of one row.
fn first_cursor(mcp: &mut McpServer, id: &mut u64) -> String {
    events_page(mcp, id, json!({"limit": 1}))
        .next_cursor
        .expect("setup: the ledger holds a single row, so no next_cursor")
}

fn seq(row: &Value) -> u64 {
    row["seq"]
        .as_u64()
        .unwrap_or_else(|| panic!("an event row without seq: {row}"))
}

fn sequences(pages: &[Page]) -> Vec<u64> {
    pages
        .iter()
        .flat_map(|page| page.rows.iter().map(seq))
        .collect()
}
