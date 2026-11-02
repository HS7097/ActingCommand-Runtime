// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-12 (register §1.264; contracts/runtime-interface.md, "Workbench request kinds",
// the row `RoutineSummary`): the routine summary through the host, on a fake clock and an injected
// host zone nine hours east of UTC.
// - direct runs that succeed, fail and are cancelled are counted as the host settles them, by the
//   task their record names: successes, failures by their code, interruptions, in the hour and
//   the day of the host's local clock that hold them;
// - a cancelled run is an interruption and never a failure, also in the form a scheduled run's
//   terminal takes (a failure that carries `contained_task_cancelled`);
// - a restart begins a new owner epoch, its revisions and every instance's coverage again;
// - every form of the read answers from memory and appends nothing to the ledger, and an alias
//   no instance has is refused as the other instance reads refuse it.
// The memory on its own (buckets across offsets, revisions, bounds) is `routine_summary::tests`'.
// Every id and code is neutral.

use super::gate_369s5::direct_run_package;
use super::gate_383_ao4q::ledger_head;
use super::*;
use crate::routine_summary::RunEnd;
use actingcommand_contract::outcome::CodeStr;
use actingcommand_contract::{
    InstanceId, RoutineSummaryFailure, RoutineSummaryGranularity, RoutineSummaryPage,
    RuntimeErrorCode, RuntimeStartReport,
};

const ALIAS: &str = "r12.instance.a";
/// 2026-10-26 07:01 UTC: 16:01 nine hours east, early in a local hour.
const START_UNIX_MS: u64 = 1_792_998_060_000;
const HOUR_MS: u64 = 60 * 60 * 1_000;
const DAY_MS: u64 = 24 * HOUR_MS;
/// The host's injected zone: nine hours east of UTC.
const EAST_NINE_MS: u64 = 9 * HOUR_MS;

/// The start of the local hour or day (`width_ms`), nine hours east, that holds `unix_ms`.
fn local_start(unix_ms: u64, width_ms: u64) -> u64 {
    (unix_ms + EAST_NINE_MS) / width_ms * width_ms - EAST_NINE_MS
}

/// A host on `clock` whose zone is nine hours east of UTC, with the one instance `ALIAS`.
fn start_host(
    root: &TempDir,
    clock: &Arc<ManualRuntimeClock>,
    instance: InstanceId,
    state: Arc<FakeState>,
) -> RuntimeHost {
    let runtime_clock: Arc<dyn RuntimeClock> = clock.clone();
    let host = RuntimeHost::start(
        config(root).with_runtime_clock(runtime_clock),
        Arc::new(FakeProvider::one(ALIAS, instance, state)),
    )
    .expect("runtime host");
    host.routine_summary_set_local_offset_for_test(i64::try_from(EAST_NINE_MS).expect("an offset"))
        .expect("the host's zone");
    host
}

/// The page `RoutineSummary` answers on `client`, a completed receipt with its own success code.
fn summary(
    client: &mut TestClient,
    instance: Option<&str>,
    granularity: RoutineSummaryGranularity,
    since_revision: Option<u64>,
) -> RoutineSummaryPage {
    let request = client.request(RuntimeOperation::RoutineSummary {
        instance: instance.map(str::to_owned),
        granularity,
        since_revision,
    });
    let receipt = client.send(&request);
    assert_eq!(
        receipt.state(),
        RuntimeReceiptState::Completed,
        "{receipt:?}"
    );
    assert_eq!(
        receipt.outcome().code().as_str(),
        "request_routine_summary_completed"
    );
    let Some(RuntimeResult::RoutineSummary { summary }) = receipt.result() else {
        panic!("a routine summary: {receipt:?}");
    };
    (**summary).clone()
}

/// What the Runtime says of its own start: its owner epoch and when it began.
fn start_report(client: &mut TestClient) -> RuntimeStartReport {
    let request = client.request(RuntimeOperation::StartReport);
    let receipt = client.send(&request);
    let Some(RuntimeResult::StartReport { report }) = receipt.result() else {
        panic!("a start report: {receipt:?}");
    };
    (**report).clone()
}

/// How a direct run of the neutral package is made to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Success,
    /// Its captures stay unknown after its admission.
    Failure,
    /// It is cancelled on another connection after its admission.
    Cancel,
}

/// Direct runs of the neutral package on `ALIAS`.
struct Runs<'a> {
    host: &'a RuntimeHost,
    state: &'a Arc<FakeState>,
    package: &'a str,
    instance: InstanceId,
    ids: IdentifierIssuer,
}

impl Runs<'_> {
    /// One direct run on `connection`, ended as `end` asks: its receipt.
    fn run(&self, connection: u64, end: End) -> RuntimeReceipt {
        // Each run starts on the home page: no input yet, captures known.
        self.state.input_count.store(0, Ordering::Release);
        self.state.unknown_capture.store(false, Ordering::Release);
        let request = runtime_request(
            &self.ids,
            RuntimeOperation::run_contained_task(
                ALIAS,
                self.ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(self.package).expect("task request"),
            ),
        );
        let run = request.request_id();
        let address = self
            .host
            .runtime_info()
            .socket_addr()
            .expect("runtime address");
        let state = Arc::clone(self.state);
        let checkpoint = self
            .host
            .run_at_contained_task_checkpoint_for_test(run, self.instance, None, move |_| match end
            {
                End::Success => {}
                End::Failure => state.unknown_capture.store(true, Ordering::Release),
                End::Cancel => {
                    let mut canceller = TestClient::connect_address(address);
                    let cancel = canceller.request(RuntimeOperation::CancelContainedTask {
                        task_request_id: run,
                    });
                    let receipt = canceller.send(&cancel);
                    assert_eq!(
                        receipt.state(),
                        RuntimeReceiptState::Completed,
                        "{receipt:?}"
                    );
                }
            })
            .expect("arm the run's checkpoint");
        let receipt = self
            .host
            .process_request_for_test(&request, ConnectionId::new(connection).expect("connection"))
            .expect("the run's receipt");
        assert_eq!(checkpoint.consumed(), 1, "{end:?}: the run was admitted");
        receipt
    }
}

// Two successes, a failure and a cancel of one task: the hour and the day of the host's local
// clock that hold them count two successes, the failure under the code its receipt carries, and
// one interruption; the page's revision and each bucket's are the fourth change, and the coverage
// is the Runtime's start.
#[test]
fn routine_summary_counts_successes_failures_by_code_and_interruptions() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(START_UNIX_MS, 0));
    let host = start_host(&root, &clock, instance, Arc::clone(&state));
    let runs = Runs {
        host: &host,
        state: &state,
        package: &package,
        instance,
        ids: IdentifierIssuer::new().expect("identifier issuer"),
    };
    let mut failure_code = None;
    for (connection, end) in
        (1_201_u64..).zip([End::Success, End::Success, End::Failure, End::Cancel])
    {
        let receipt = runs.run(connection, end);
        match end {
            End::Success => assert!(
                matches!(
                    receipt.result(),
                    Some(RuntimeResult::ContainedTaskCompleted {
                        task_result: TaskOutcome::Success,
                        ..
                    })
                ),
                "{receipt:?}"
            ),
            End::Failure => {
                assert!(
                    !matches!(
                        receipt.result(),
                        Some(
                            RuntimeResult::ContainedTaskCompleted { .. }
                                | RuntimeResult::ContainedTaskCancelled { .. }
                        )
                    ),
                    "{receipt:?}"
                );
                failure_code = Some(receipt.outcome().code().clone());
            }
            End::Cancel => assert!(
                matches!(
                    receipt.result(),
                    Some(RuntimeResult::ContainedTaskCancelled { .. })
                ),
                "{receipt:?}"
            ),
        }
    }
    let failure_code = failure_code.expect("the failure's code");
    assert_ne!(
        failure_code,
        CodeStr::from(RuntimeErrorCode::ContainedTaskCancelled.code())
    );

    let mut reader = TestClient::connect(&host);
    let report = start_report(&mut reader);
    for (granularity, width_ms) in [
        (RoutineSummaryGranularity::Hour, HOUR_MS),
        (RoutineSummaryGranularity::Day, DAY_MS),
    ] {
        let page = summary(&mut reader, None, granularity, None);
        assert_eq!(page.owner_epoch, report.owner_epoch);
        assert_eq!(page.granularity, granularity);
        assert_eq!(page.revision, 4, "{page:?}");
        assert_eq!(page.instances.len(), 1, "{page:?}");
        let counted = &page.instances[0];
        assert_eq!(counted.instance_alias, ALIAS);
        assert_eq!(counted.coverage_from_unix_ms, report.started_at_unix_ms);
        assert_eq!(counted.buckets.len(), 1, "{granularity:?}: {page:?}");
        let bucket = &counted.buckets[0];
        let start = local_start(START_UNIX_MS, width_ms);
        assert_eq!(
            (bucket.start_unix_ms, bucket.end_unix_ms),
            (start, start + width_ms),
            "{granularity:?}"
        );
        assert_eq!(bucket.revision, 4);
        assert_eq!(bucket.tasks.len(), 1, "one task: {bucket:?}");
        let task = &bucket.tasks[0];
        assert!(!task.task_id.is_empty());
        assert_eq!(task.successes, 2, "{granularity:?}");
        assert_eq!(
            task.failures,
            [RoutineSummaryFailure {
                code: failure_code.clone(),
                count: 1,
            }],
            "{granularity:?}"
        );
        assert_eq!(task.interruptions, 1, "{granularity:?}");
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(reader);
    host.close().expect("close host");
}

// A direct run cancelled by `CancelContainedTask` is one interruption, no failure: its task row
// has no `failures` on the wire. A scheduled run's cancel ends in a failure terminal that carries
// `contained_task_cancelled`: that is an interruption too, while another code is a failure.
#[test]
fn a_cancelled_run_is_an_interruption_never_a_failure() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let instance = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(START_UNIX_MS, 0));
    let host = start_host(&root, &clock, instance, Arc::clone(&state));
    let runs = Runs {
        host: &host,
        state: &state,
        package: &package,
        instance,
        ids: IdentifierIssuer::new().expect("identifier issuer"),
    };
    let receipt = runs.run(1_301, End::Cancel);
    assert!(
        matches!(
            receipt.result(),
            Some(RuntimeResult::ContainedTaskCancelled { .. })
        ),
        "{receipt:?}"
    );

    let mut reader = TestClient::connect(&host);
    for granularity in [
        RoutineSummaryGranularity::Hour,
        RoutineSummaryGranularity::Day,
    ] {
        let page = summary(&mut reader, Some(ALIAS), granularity, None);
        let task = &page.instances[0].buckets[0].tasks[0];
        assert_eq!(
            (task.successes, task.failures.len(), task.interruptions),
            (0, 0, 1),
            "{granularity:?}: {page:?}"
        );
        let wire = serde_json::to_value(&page).expect("the page encodes");
        assert!(
            wire["instances"][0]["buckets"][0]["tasks"][0]
                .get("failures")
                .is_none(),
            "{wire}"
        );
    }

    // The scheduled form: a failure terminal that carries the cancel's code.
    let cancelled = CodeStr::from(RuntimeErrorCode::ContainedTaskCancelled.code());
    assert_eq!(RunEnd::of_terminal(Some(cancelled)), RunEnd::Interrupted);
    let deadline = CodeStr::from(RuntimeErrorCode::ContainedTaskDeadlineExceeded.code());
    assert_eq!(
        RunEnd::of_terminal(Some(deadline.clone())),
        RunEnd::Failed(deadline)
    );
    assert_eq!(RunEnd::of_terminal(None), RunEnd::Succeeded);
    drop(reader);
    host.close().expect("close host");
}

// A restart begins a new owner epoch: the summary's revision starts at 0 again, no bucket of the
// earlier epoch is answered, and the instance's coverage is the new start; a cursor of the
// earlier epoch answers nothing, and the page's `owner_epoch` tells the client to drop it.
#[test]
fn a_restart_begins_a_new_epoch_and_its_coverage() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(START_UNIX_MS, 0));
    let host = start_host(&root, &clock, instance_id(), Arc::new(FakeState::default()));
    host.routine_summary_count_for_test(ALIAS, "task.a", RunEnd::Succeeded)
        .expect("a counted run");
    let mut reader = TestClient::connect(&host);
    let first = start_report(&mut reader);
    let before = summary(&mut reader, None, RoutineSummaryGranularity::Hour, None);
    assert_eq!(before.owner_epoch, first.owner_epoch);
    assert_eq!(before.revision, 1);
    assert_eq!(before.instances[0].buckets.len(), 1);
    assert_eq!(
        before.instances[0].coverage_from_unix_ms,
        first.started_at_unix_ms
    );
    drop(reader);
    host.close().expect("close host");

    clock.advance(HOUR_MS);
    let host = start_host(&root, &clock, instance_id(), Arc::new(FakeState::default()));
    let mut reader = TestClient::connect(&host);
    let second = start_report(&mut reader);
    assert_ne!(second.owner_epoch, first.owner_epoch);
    assert!(second.started_at_unix_ms >= first.started_at_unix_ms + HOUR_MS);
    for granularity in [
        RoutineSummaryGranularity::Hour,
        RoutineSummaryGranularity::Day,
    ] {
        let after = summary(&mut reader, None, granularity, None);
        assert_eq!(after.owner_epoch, second.owner_epoch);
        assert_eq!(after.revision, 0);
        assert_eq!(after.instances.len(), 1);
        assert_eq!(after.instances[0].instance_alias, ALIAS);
        assert!(after.instances[0].buckets.is_empty(), "{after:?}");
        assert_eq!(
            after.instances[0].coverage_from_unix_ms,
            second.started_at_unix_ms
        );
        // The earlier epoch's cursor.
        let stale = summary(&mut reader, None, granularity, Some(before.revision));
        assert_eq!(stale.owner_epoch, second.owner_epoch);
        assert_ne!(stale.owner_epoch, before.owner_epoch);
        assert!(stale.instances[0].buckets.is_empty(), "{stale:?}");
    }
    drop(reader);
    host.close().expect("close host");
}

// Every form of the read (each width, every instance or the one, with and without a cursor)
// answers from memory: the ledger's committed head does not move. An alias no instance has is
// Denied `instance_unknown` with no result, as `ResourceTargetView` refuses it, and records
// nothing either.
#[test]
fn routine_summary_reads_nothing_from_the_ledger() {
    let root = TempDir::new().expect("tempdir");
    let clock = Arc::new(ManualRuntimeClock::new(START_UNIX_MS, 0));
    let host = start_host(&root, &clock, instance_id(), Arc::new(FakeState::default()));
    host.routine_summary_count_for_test(ALIAS, "task.a", RunEnd::Succeeded)
        .expect("a counted run");
    let mut reader = TestClient::connect(&host);
    let head = ledger_head(&mut reader);

    for granularity in [
        RoutineSummaryGranularity::Hour,
        RoutineSummaryGranularity::Day,
    ] {
        for since_revision in [None, Some(0), Some(1), Some(7)] {
            let every = summary(&mut reader, None, granularity, since_revision);
            let one = summary(&mut reader, Some(ALIAS), granularity, since_revision);
            assert_eq!(one, every, "{granularity:?} {since_revision:?}");
            assert_eq!(
                every.instances[0].buckets.len(),
                usize::from(since_revision.is_none_or(|since| since < 1)),
                "{granularity:?} {since_revision:?}"
            );
        }
    }

    let unknown = "r12.instance.unknown";
    let request = reader.request(RuntimeOperation::RoutineSummary {
        instance: Some(unknown.to_owned()),
        granularity: RoutineSummaryGranularity::Day,
        since_revision: None,
    });
    let refused = reader.send(&request);
    assert_eq!(refused.state(), RuntimeReceiptState::Denied, "{refused:?}");
    assert_eq!(refused.outcome().code().as_str(), "instance_unknown");
    assert!(refused.result().is_none());
    let request = reader.request(RuntimeOperation::ResourceTargetView {
        instance_alias: unknown.to_owned(),
    });
    let view = reader.send(&request);
    assert_eq!(view.state(), refused.state(), "{view:?}");
    assert_eq!(view.outcome().code(), refused.outcome().code());

    assert_eq!(ledger_head(&mut reader), head);
    drop(reader);
    host.close().expect("close host");
}
