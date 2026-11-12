// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #434 LAB-5G1b (register §1.287, "Slice LAB-5G1b"; coordinator work order
// rd-1112/brief-lab5g1b.md): the code that serves the core rows (`lab_ops::core_ops`) takes the
// answer budget and the cursor binding from its caller (`Caller::budget`, `Caller::cursor`);
// actinglab passes no budget and a fresh binding for each call, so its lines do not move.
// - `lab5g1b_a_budget_cuts_or_exports_and_none_is_the_command_lines_line`: with no budget a row
//   answers the line the command line prints; with a budget that refuses the whole answer a row
//   that cuts cuts (`codes`: one row kept, `truncated`, the cursor after it) and a row that exports
//   exports (`packs`: `{source, export}` with `answer_exported` the line's one warning, the file
//   holding the whole document), and one with no export answers whole.
// - `lab5g1b_the_callers_cursor_binding_binds_events_and_no_other_cursor`: a cursor `events`
//   issues under the caller's binding names that binding, is taken under it and refused
//   (`cursor_invalid`, nothing read) under another; `codes` answers the same line, its cursor
//   bound to no session, whatever the binding, and the cursor is taken as before; `runs` refuses a
//   cursor of its own form the same way under either binding.
// - `lab5g1b_the_errors_page_of_diagnose_is_bound_to_the_callers_binding`: the cursor of the
//   errors page a `diagnose` answer carries names the caller's binding; `events` of the errors
//   view of the diagnosis' window takes it under that binding and refuses it (`cursor_invalid`,
//   nothing read) under another.
// Fakes only: the runtime client's scripted Runtime on loopback in a temporary state root, made-up
// ledger records, a temporary configuration; no device, no clock, no sleep (`diagnose` takes its
// window from its own clock, and the test reads the window from its answer).

use super::*;
use crate::cli_module::{self, words};
use actingcommand_contract::{
    AuditInput, EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin,
    EventPayloadDraft, EventSeverity, EventSource, IdentifierIssuer, OriginModule, ProjectedEvent,
    ProjectionPayload, RuntimeErrorCode, RuntimeErrorProjection, RuntimeEventQueryCursor,
    RuntimeEventQueryPage, RuntimeOperation, RuntimePayloadDraft, RuntimeReceiptState,
    RuntimeRequest, RuntimeResult, SanitizationError, SecretField, SecretFingerprinter,
    Sha256Fingerprint,
};
use actingcommand_lab_ops::budget::Budget;
use actingcommand_lab_ops::cli_line::Exit;
use actingcommand_lab_ops::code_lookup;
use actingcommand_lab_ops::core_ops::{self, Caller, Place};
use actingcommand_lab_ops::events::{self, CursorBinding};
use actingcommand_lab_ops::front::Connection;
use actingcommand_lab_ops::initiator::ENVIRONMENT_VARIABLE;
use actingcommand_lab_ops::material_export;
use actingcommand_lab_ops::table::{Operation, OperationTable};
use actingcommand_runtime_client::test_support::{
    ScriptedAnswer, ScriptedConnection, ScriptedRuntime,
};
use serde_json::{Map, Value, json};
use std::ffi::OsString;
use std::path::Path;

/// The caller's binding, and another one: the same session, another connection generation.
const BOUND: CursorBinding = CursorBinding {
    session: 0x5eed,
    generation: 7,
};
const OTHER: CursorBinding = CursorBinding {
    session: 0x5eed,
    generation: 8,
};
/// The time of the made-up record at sequence 0; a record's time is this plus its sequence.
const RECORDED_FROM_UNIX_MS: u64 = 1_700_000_000_000;
/// A configuration with one instance and no package binding.
const ONE_INSTANCE: &str = r#"{"instances": [{"alias": "neutral.a", "instance_id": "neutral-a"}]}"#;

fn words_of(line: &[&str]) -> Vec<String> {
    line.iter().map(|word| (*word).to_owned()).collect()
}

/// The row and the parameters the command line `line` gives.
fn called<'t>(table: &'t OperationTable, line: &[&str]) -> (&'t Operation, Map<String, Value>) {
    let call = words::parse(table, &words_of(line))
        .unwrap_or_else(|outcome| panic!("{line:?} is refused: {outcome:?}"));
    let operation = table.operation(&call.name).expect("the call's row");
    assert!(core_ops::serves(operation), "{line:?}");
    (operation, call.params)
}

/// The exit code and the line.
fn printed(exit: &Exit) -> (u8, Value) {
    (exit.code, exit.line.clone().expect("a JSON line"))
}

struct NoSecrets;

impl SecretFingerprinter for NoSecrets {
    fn fingerprint(
        &self,
        _field: SecretField,
        _original: &str,
    ) -> Result<Sha256Fingerprint, SanitizationError> {
        panic!("the made-up records hold no secret")
    }
}

/// Made-up records at the sequences 1 to `count`, as a forensic read projects them.
fn ledger(count: u64) -> Vec<ProjectedEvent> {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    (1..=count)
        .map(|sequence| {
            let payload: EventPayloadDraft =
                RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into();
            let sanitized = EventDraft::new(
                issuer.mint_event_id().expect("event id"),
                RECORDED_FROM_UNIX_MS + sequence,
                EventSeverity::Info,
                EventOrigin::new(
                    EventSource::Runtime,
                    OriginModule::Runtime,
                    EventActor::Runtime,
                ),
                EventLinksDraft::default(),
                payload,
            )
            .sanitize(&NoSecrets)
            .expect("sanitize a made-up record");
            ProjectedEvent {
                views: Vec::new(),
                artifact_evictions: Vec::new(),
                schema_version: sanitized.schema_version().to_owned(),
                sequence,
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
            }
        })
        .collect()
}

/// One call's line and how many event queries its one connection received.
struct Served {
    code: u8,
    line: Value,
    queries: usize,
}

/// `run` (given the state root) on a scripted Runtime whose one connection answers its event
/// queries from `records`; the call makes that one connection.
fn on_ledger(records: Vec<ProjectedEvent>, run: impl FnOnce(&Path) -> Exit) -> Served {
    served_on(ScriptedConnection::new().ledger(records), run)
}

/// `run` (given the state root) on a scripted Runtime whose one connection answers as `script`
/// says; the call makes that one connection.
fn served_on(script: ScriptedConnection, run: impl FnOnce(&Path) -> Exit) -> Served {
    let temp = TempDir::new().unwrap();
    let state = temp.path().join("state");
    fs::create_dir_all(&state).unwrap();
    let runtime = ScriptedRuntime::start(&state, |_| vec![script]);
    let exit = {
        let _state_root = use_runtime_state_root(&state);
        run(&state)
    };
    let record = runtime.finish();
    assert!(
        record.unscripted.is_empty(),
        "a connection beyond the call's one"
    );
    let queries = record
        .connections
        .iter()
        .map(|connection| {
            connection.count(|operation| matches!(operation, RuntimeOperation::QueryEvents { .. }))
        })
        .sum();
    let (code, line) = printed(&exit);
    Served {
        code,
        line,
        queries,
    }
}

/// The words of `events` that take `cursor`, on the state root `state`.
fn cursor_words(state: &Path, cursor: &str) -> Vec<OsString> {
    vec![
        OsString::from("events"),
        OsString::from("--state-root"),
        state.as_os_str().to_owned(),
        OsString::from("--cursor"),
        OsString::from(cursor),
    ]
}

/// The words of `events` that take `cursor` of `diagnose`'s errors page: the errors view of the
/// diagnosis' window, from `since_unix_ms`, on the state root `state`.
fn errors_words(state: &Path, since_unix_ms: u64, cursor: &str) -> Vec<OsString> {
    vec![
        OsString::from("events"),
        OsString::from("--state-root"),
        state.as_os_str().to_owned(),
        OsString::from("--view"),
        OsString::from("errors"),
        OsString::from("--since-unix-ms"),
        OsString::from(since_unix_ms.to_string()),
        OsString::from("--cursor"),
        OsString::from(cursor),
    ]
}

/// A refusal of the Runtime: denied, `lease_busy`; the diagnosis goes on without the part it
/// refused.
fn denied(request: &RuntimeRequest) -> ScriptedAnswer {
    ScriptedAnswer::failed(
        request,
        RuntimeReceiptState::Denied,
        RuntimeErrorProjection::new(RuntimeErrorCode::LeaseBusy, false).receipt_outcome(None, None),
    )
}

/// A diagnosis' errors page: the one made-up `row`, at a snapshot one past it, with the cursor
/// after it to the rest of the snapshot, for the query the page was asked for.
fn errors_page(
    row: ProjectedEvent,
) -> impl FnOnce(&RuntimeRequest) -> ScriptedAnswer + Send + 'static {
    move |request| {
        let RuntimeOperation::QueryEvents {
            query,
            profile,
            page,
        } = request.operation()
        else {
            panic!(
                "the errors page is read first, got {:?}",
                request.operation()
            );
        };
        let snapshot = row.sequence + 1;
        let next = RuntimeEventQueryCursor::new(snapshot, row.sequence, query, *profile)
            .expect("the errors page's next cursor");
        let page = RuntimeEventQueryPage::new(vec![row], snapshot, page.limit(), true, Some(next))
            .expect("the errors page");
        ScriptedAnswer::completed(request, RuntimeResult::EventPage { page })
    }
}

/// The page a cursor continues to: empty, at the cursor's snapshot; a read without a cursor fails
/// the script.
fn rest_of_snapshot(request: &RuntimeRequest) -> ScriptedAnswer {
    let RuntimeOperation::QueryEvents { page, .. } = request.operation() else {
        panic!("a page is read, got {:?}", request.operation());
    };
    let snapshot = page
        .cursor()
        .expect("the read goes on from the cursor it was given")
        .snapshot_ledger_position();
    let page = RuntimeEventQueryPage::new(Vec::new(), snapshot, page.limit(), false, None)
        .expect("the rest of the snapshot");
    ScriptedAnswer::completed(request, RuntimeResult::EventPage { page })
}

/// The seq of each row of an `events` line.
fn sequences(line: &Value) -> Vec<u64> {
    line["events"]
        .as_array()
        .unwrap_or_else(|| panic!("a line without events: {line}"))
        .iter()
        .map(|row| row["seq"].as_u64().expect("a row's seq"))
        .collect()
}

#[test]
fn lab5g1b_a_budget_cuts_or_exports_and_none_is_the_command_lines_line() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let table = cli_module::table().expect("the operation table");

    // `codes`, a row that cuts, offline: no budget answers the command line's line, every row of
    // the page; a budget that takes one row keeps the first, marks the cut and continues after it.
    let line = ["codes", "--prefix", "actingctl_", "--limit", "3"];
    let (operation, params) = called(table, &line);
    let whole = printed(&core_ops::answer(
        operation,
        &params,
        &cli_module::core_ops::caller(),
    ));
    assert_eq!(
        whole,
        printed(&cli_module::run_line(&words_of(&line))),
        "{line:?}"
    );
    let (code, whole) = whole;
    assert_eq!(
        (code, &whole["outcome"]["code"]),
        (0, &json!("outcome_codes_reported")),
        "{whole}"
    );
    let rows = whole["codes"].as_array().expect("the page's rows").clone();
    assert_eq!(rows.len(), 3, "{whole}");
    assert!(whole.get("truncated").is_none(), "{whole}");
    let one_row = |answer: &Value| {
        answer["codes"]
            .as_array()
            .is_some_and(|codes| codes.len() <= 1)
    };
    let budget = Budget::new(&one_row);
    let (code, cut) = printed(&core_ops::answer(
        operation,
        &params,
        &Caller {
            budget: Some(&budget),
            ..cli_module::core_ops::caller()
        },
    ));
    assert_eq!(
        (code, &cut["outcome"]["code"]),
        (0, &json!("outcome_codes_reported")),
        "{cut}"
    );
    assert_eq!(cut["codes"], json!([rows[0].clone()]), "{cut}");
    assert_eq!(cut["truncated"], json!(true), "{cut}");
    assert!(cut["next_cursor"].is_string(), "{cut}");

    // `packs`, a row that exports, offline: the configuration of a made-up root.
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("actingd.config.json"), ONE_INSTANCE).unwrap();
    let exports = temp.path().join("exports");
    let (place_state, place_root) = (temp.path().join("state"), root.clone());
    let here = move || Place {
        state_root: Ok(place_state.clone()),
        root: Some(place_root.clone()),
    };
    let (operation, params) = called(table, &["packs"]);
    let at_root = |budget: Option<&Budget<'_>>| {
        printed(&core_ops::answer(
            operation,
            &params,
            &Caller {
                place: &here,
                budget,
                ..cli_module::core_ops::caller()
            },
        ))
    };
    let (code, whole) = at_root(None);
    assert_eq!(
        (code, &whole["outcome"]["code"]),
        (0, &json!("package_bindings_reported")),
        "{whole}"
    );
    assert_eq!(
        whole["instances"][0]["alias"],
        json!("neutral.a"),
        "{whole}"
    );
    assert!(whole.get("export").is_none(), "{whole}");
    let today = packs_line(&root);
    assert_eq!(
        (code, &whole),
        (today.0, &today.1),
        "no budget: the command line's line"
    );
    // A budget that takes nothing, with an export: the whole document written, its header
    // answered with `answer_exported` its one warning.
    let nothing = |_: &Value| false;
    let export = |sha256: &str, extension: &str, bytes: &[u8]| {
        material_export::write(&exports, sha256, extension, bytes)
            .map_err(|failure| failure.outcome())
    };
    let exporting = Budget::new(&nothing).with_export(&export);
    let (code, exported) = at_root(Some(&exporting));
    assert_eq!(
        (code, &exported["outcome"]["code"]),
        (0, &json!("package_bindings_reported")),
        "{exported}"
    );
    assert_eq!(exported["source"], whole["source"], "{exported}");
    assert!(exported.get("instances").is_none(), "{exported}");
    let warnings = exported["warnings"]
        .as_array()
        .expect("the line's warnings");
    assert_eq!(warnings.len(), 1, "{exported}");
    assert_eq!(
        warnings[0]["outcome"]["code"],
        json!("answer_exported"),
        "{exported}"
    );
    let path = exported["export"]["path"]
        .as_str()
        .expect("the export's path");
    let file: Value =
        serde_json::from_slice(&fs::read(path).expect("the exported file")).expect("JSON");
    let mut document = whole.clone();
    document
        .as_object_mut()
        .expect("a line is an object")
        .remove("outcome");
    assert_eq!(file, document, "the file holds the whole document");
    // The same budget without an export: the whole answer, for the caller to refuse.
    let no_export = Budget::new(&nothing);
    assert_eq!(at_root(Some(&no_export)), (code, whole));
}

/// `packs --root <root>` as `lab-ops`' command answers it (the command line's words).
fn packs_line(root: &Path) -> (u8, Value) {
    let argv = [
        OsString::from("packs"),
        OsString::from("--root"),
        root.as_os_str().to_owned(),
    ];
    printed(&actingcommand_lab_ops::packs::dispatch(&argv).expect("the words of packs"))
}

#[test]
fn lab5g1b_the_callers_cursor_binding_binds_events_and_no_other_cursor() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let table = cli_module::table().expect("the operation table");

    // `events`: a page of two of four records issues its next cursor under the caller's binding.
    let (operation, params) = called(table, &["events", "--after", "0", "--limit", "2"]);
    let issued = on_ledger(ledger(4), |_| {
        core_ops::answer(
            operation,
            &params,
            &Caller {
                cursor: BOUND,
                ..cli_module::core_ops::caller()
            },
        )
    });
    assert_eq!(
        (issued.code, &issued.line["outcome"]["code"]),
        (0, &json!("runtime_events_reported")),
        "{}",
        issued.line
    );
    assert_eq!(sequences(&issued.line), [1, 2], "{}", issued.line);
    let cursor = issued.line["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("no next_cursor: {}", issued.line))
        .to_owned();
    let bound = format!("v1.{:016x}.{}.", BOUND.session, BOUND.generation);
    assert!(
        cursor.starts_with(&bound),
        "{cursor} is not bound to {bound}"
    );

    // Taken under the binding it was issued under: the rest of the snapshot.
    let taken = on_ledger(ledger(4), |state| {
        events::dispatch_within(
            &cursor_words(state, &cursor),
            Connection::per_call(),
            None,
            BOUND,
        )
        .expect("the words of events")
    });
    assert_eq!(
        (taken.code, &taken.line["outcome"]["code"]),
        (0, &json!("runtime_events_reported")),
        "{}",
        taken.line
    );
    assert_eq!(sequences(&taken.line), [3, 4], "{}", taken.line);
    assert_eq!(taken.queries, 1, "{}", taken.line);

    // Under another binding: `cursor_invalid`, and nothing is read.
    let refused = on_ledger(ledger(4), |state| {
        events::dispatch_within(
            &cursor_words(state, &cursor),
            Connection::per_call(),
            None,
            OTHER,
        )
        .expect("the words of events")
    });
    assert_eq!(
        (refused.code, &refused.line["outcome"]["code"]),
        (1, &json!("cursor_invalid")),
        "{}",
        refused.line
    );
    assert_eq!(refused.queries, 0, "{}", refused.line);

    // `codes`: the same line under either binding, its cursor bound to no session; the cursor is
    // taken as before.
    let line = ["codes", "--prefix", "actingctl_", "--limit", "2"];
    let (operation, params) = called(table, &line);
    let under = |cursor: CursorBinding| {
        printed(&core_ops::answer(
            operation,
            &params,
            &Caller {
                cursor,
                ..cli_module::core_ops::caller()
            },
        ))
    };
    let first = under(BOUND);
    assert_eq!(first, under(OTHER), "{line:?}");
    assert_eq!(
        first,
        printed(&cli_module::run_line(&words_of(&line))),
        "{line:?}"
    );
    let next = first.1["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("no next_cursor: {}", first.1))
        .to_owned();
    let argv = [
        "codes",
        "--prefix",
        "actingctl_",
        "--limit",
        "2",
        "--cursor",
        next.as_str(),
    ]
    .map(OsString::from);
    let (code, second) = printed(&code_lookup::dispatch(&argv).expect("the words of codes"));
    assert_eq!(
        (code, &second["outcome"]["code"]),
        (0, &json!("outcome_codes_reported")),
        "{second}"
    );
    assert_ne!(second["codes"], first.1["codes"], "the next page");

    // `runs`: a cursor of its own form it cannot read is refused alike under either binding.
    let (operation, params) = called(table, &["runs", "--cursor", "r1.0"]);
    let refused = [BOUND, OTHER].map(|cursor| {
        on_ledger(Vec::new(), |_| {
            core_ops::answer(
                operation,
                &params,
                &Caller {
                    cursor,
                    ..cli_module::core_ops::caller()
                },
            )
        })
    });
    for served in &refused {
        assert_eq!(
            (served.code, &served.line["outcome"]["code"]),
            (1, &json!("cursor_invalid")),
            "{}",
            served.line
        );
        assert_eq!(served.queries, 0, "{}", served.line);
    }
    assert_eq!(refused[0].line, refused[1].line);
}

#[test]
fn lab5g1b_the_errors_page_of_diagnose_is_bound_to_the_callers_binding() {
    let _guard = env_lock();
    unsafe {
        env::remove_var(ENVIRONMENT_VARIABLE);
    }
    let table = cli_module::table().expect("the operation table");

    // `diagnose` of every instance, its window the command's own: the errors page (one made-up
    // row, more after it) is its first read; the Runtime refuses the four reads that follow it
    // (the failure records' first page, the schedule explanation and the runtime facts of
    // `not_run`, the window's run listing), and the diagnosis answers without those parts. No
    // instance, so no status read; no install root, so no `actingd suspended` child.
    let (operation, params) = called(table, &["diagnose"]);
    let row = ledger(1).remove(0);
    let script = ScriptedConnection::new()
        .request(errors_page(row))
        .request(denied)
        .request(denied)
        .request(denied)
        .request(denied);
    let diagnosed = served_on(script, |state| {
        let here = || Place {
            state_root: Ok(state.to_path_buf()),
            root: None,
        };
        core_ops::answer(
            operation,
            &params,
            &Caller {
                place: &here,
                cursor: BOUND,
                ..cli_module::core_ops::caller()
            },
        )
    });
    assert_eq!(
        (diagnosed.code, &diagnosed.line["outcome"]["code"]),
        (0, &json!("runtime_diagnosis_reported")),
        "{}",
        diagnosed.line
    );
    assert_eq!(diagnosed.queries, 3, "{}", diagnosed.line);
    let cursor = diagnosed.line["errors_page"]["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("no errors page cursor: {}", diagnosed.line))
        .to_owned();
    let bound = format!("v1.{:016x}.{}.", BOUND.session, BOUND.generation);
    assert!(
        cursor.starts_with(&bound),
        "{cursor} is not bound to {bound}"
    );
    let since = diagnosed.line["window"]["since_unix_ms"]
        .as_u64()
        .unwrap_or_else(|| panic!("no window: {}", diagnosed.line));

    // `events` of the errors view of the same window takes it under the same binding: one read,
    // from the cursor.
    let taken = served_on(
        ScriptedConnection::new().request(rest_of_snapshot),
        |state| {
            events::dispatch_within(
                &errors_words(state, since, &cursor),
                Connection::per_call(),
                None,
                BOUND,
            )
            .expect("the words of events")
        },
    );
    assert_eq!(
        (taken.code, &taken.line["outcome"]["code"]),
        (0, &json!("runtime_events_reported")),
        "{}",
        taken.line
    );
    assert_eq!(taken.queries, 1, "{}", taken.line);

    // Under another binding: `cursor_invalid`, and nothing is read.
    let refused = served_on(ScriptedConnection::new(), |state| {
        events::dispatch_within(
            &errors_words(state, since, &cursor),
            Connection::per_call(),
            None,
            OTHER,
        )
        .expect("the words of events")
    });
    assert_eq!(
        (refused.code, &refused.line["outcome"]["code"]),
        (1, &json!("cursor_invalid")),
        "{}",
        refused.line
    );
    assert_eq!(refused.queries, 0, "{}", refused.line);
}
