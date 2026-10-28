// SPDX-License-Identifier: AGPL-3.0-only

//! #422 QR-2 (v0.12.23; brief rd-1028/brief-qr2-overview-pages.md, investigation
//! rd-1028/qr2-investigation.md §2.1 and §4 R4). On production v0.12.21 the overview's run block
//! cost 5 to 28 s per instance and `ac_diagnose` failed at the client's 5 s socket timeout: every
//! QueryEvents page of the run listing and of a run's status asked the Runtime for the page's
//! `run_recovery`, which reads up to 1 025 records of every run the page names, and no reader in
//! this repository reads it. The required state, the client's part:
//! - T3: the overview's run block, the run listing (`ac_runs`) and a run's status (`ac_get_run`,
//!   by its request and by its run) ask for every QueryEvents page without run recovery, and
//!   answer as before: the newest run, the window's runs newest first, the run's state.
//!
//! The Runtime is A42's fake (`gate_411_a42`), which counts the pages asked for with their run
//! recovery.

use super::gate_411_a42::{FakeRuntime, Recorder};
use super::gate_mcpt1::unreached_deadline;
use crate::overview::{self, RunWindow, Warning};
use actingcommand_contract::{
    EventActor, EventSource, IssuedInstanceId, ProjectedEvent, RuntimeInstanceStatus,
};
use actingcommand_runtime_client::{
    ContainedRunState, RunKey, RunListFilter, RunStatusMode, RuntimeChannel, RuntimeClient,
    RuntimeClientConfig,
};
use serde_json::{Value, json};
use std::path::Path;
use tempfile::TempDir;

/// The run window (24 h).
const WINDOW_START_UNIX_MS: u64 = 4_102_444_800_000 + 2_000_000_000;
const WINDOW_END_UNIX_MS: u64 = WINDOW_START_UNIX_MS + 24 * 60 * 60 * 1000;
/// The instance's runs in the window, each with this many entry facts before its main admission
/// and its terminal.
const RUNS: usize = 6;
const ENTRY_FACTS: usize = 3;

/// One run of the instance before the window; then in the window, `RUNS` runs of the instance,
/// another instance's run before each. The instance's id, the ledger and the request_ids of the
/// instance's runs in the window, oldest first.
fn ledger() -> (IssuedInstanceId, Vec<ProjectedEvent>, Vec<Value>) {
    let mut ledger = Recorder::new(WINDOW_START_UNIX_MS - 60 * 60 * 1000);
    let instance = ledger.issuer.mint_instance_id().expect("instance id");
    let other = ledger.issuer.mint_instance_id().expect("instance id");
    ledger.entry_run(instance, ENTRY_FACTS, true);
    ledger.at(WINDOW_START_UNIX_MS);
    let mut runs = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        ledger.listed_run(other);
        runs.push(ledger.entry_run(instance, ENTRY_FACTS, true));
    }
    (instance, ledger.events, runs)
}

/// A CLI client of the fake Runtime under `root`.
fn connect_client(root: &Path) -> RuntimeClient {
    RuntimeClient::connect(RuntimeClientConfig::new(
        root,
        RuntimeChannel::Cli,
        EventActor::Cli,
        EventSource::Cli,
    ))
    .expect("connect to the fake Runtime")
}

/// The instance's overview row's run members, its warnings and whether the overview is
/// incomplete.
fn run_row(client: &RuntimeClient, instance: IssuedInstanceId) -> (Value, Vec<Warning>, bool) {
    let status = RuntimeInstanceStatus::new(
        "neutral.instance",
        *instance.transport(),
        false,
        0,
        false,
        false,
        false,
    )
    .expect("instance status");
    let mut row = json!({});
    let (mut warnings, mut incomplete) = (Vec::new(), false);
    overview::run_block(
        client,
        &status,
        RunWindow {
            since_unix_ms: WINDOW_START_UNIX_MS,
            until_unix_ms: WINDOW_END_UNIX_MS,
        },
        unreached_deadline(),
        &mut row,
        &mut warnings,
        &mut incomplete,
    );
    (row, warnings, incomplete)
}

#[test]
fn qr2_t3_the_run_block_listing_and_status_ask_for_no_run_recovery_and_answer_as_before() {
    let (instance, events, runs) = ledger();
    let root = TempDir::new().expect("tempdir");
    let runtime = FakeRuntime::start(root.path(), events);
    let client = connect_client(root.path());

    // The overview's run block: the window's newest run, complete.
    let (row, warnings, incomplete) = run_row(&client, instance);
    assert_eq!(row["run"]["request_id"], runs[RUNS - 1], "{row}");
    assert_eq!(row["run"]["state"], json!("succeeded"), "{row}");
    assert!(
        !incomplete && warnings.is_empty(),
        "incomplete {incomplete}, warnings {}",
        warnings.len()
    );

    // The run listing: every run of the instance in the window, newest first, each succeeded.
    let page = client
        .list_contained_runs(
            WINDOW_START_UNIX_MS,
            Some(WINDOW_END_UNIX_MS),
            &RunListFilter {
                instance_id: Some(*instance.transport()),
                ..RunListFilter::default()
            },
            None,
            RUNS + 1,
            unreached_deadline(),
        )
        .expect("the run listing");
    assert!(!page.incomplete && page.next.is_none(), "{page:?}");
    let listed = page
        .runs
        .iter()
        .map(|run| json!(run.status.request_id))
        .collect::<Vec<_>>();
    let newest_first = runs.iter().rev().cloned().collect::<Vec<_>>();
    assert_eq!(listed, newest_first, "the listed runs, newest first");
    assert!(
        page.runs
            .iter()
            .all(|run| run.status.state == ContainedRunState::Succeeded),
        "{page:?}"
    );

    // A run's status, by its request and by its run: the newest run, succeeded, the same answer.
    let newest = &page.runs[0].status;
    let by_request = client
        .contained_run_status(
            RunKey::RequestId(newest.request_id.expect("the run's request")),
            RunStatusMode::Full,
        )
        .expect("the status by request");
    let by_run = client
        .contained_run_status(
            RunKey::RunId(newest.run_id.expect("the run's run id")),
            RunStatusMode::Full,
        )
        .expect("the status by run");
    assert_eq!(json!(by_request.request_id), runs[RUNS - 1]);
    assert_eq!(
        by_request.state,
        ContainedRunState::Succeeded,
        "{by_request:?}"
    );
    assert_eq!(by_run, by_request, "the status by run and by request");

    // Every QueryEvents page of these reads asked for no run recovery.
    let stats = runtime.stats();
    assert!(stats.pages > 0, "the reads asked for pages: {stats:?}");
    assert_eq!(
        stats.pages_with_run_recovery, 0,
        "pages asked for with their run recovery: {stats:?}"
    );
    drop(client);
    drop(runtime);
}
