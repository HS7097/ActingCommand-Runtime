// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #429 R-0b (register §1.238; contracts/runtime-interface.md, "Workbench request kinds",
// the row `RuntimeInstanceStatus.activity?` and "What an instance does now"): the Status member
// `activity`, read through `Status` itself while each activity runs and after it ended.
// - a direct run shows `direct_run` with its request and its package from its grant, its
//   `since_unix_ms` the host clock's (a fake one), and is gone after it succeeds, fails or is
//   cancelled; an instance doing nothing has no `activity` key;
// - a policy run shows `policy_run` with the request it runs as and its package; after its
//   failure its ladder shows `ladder`, also while the ladder's emulator restart, which runs on the
//   ladder's key, is driving (the overlap rule), and nothing shows after the ladder;
// - an emulator control shows `emulator_control` with its request while it drives; a self-check
//   shows `selfcheck` with its request and a resume's reconnect `reconnect` with the resume's
//   request, each while it prepares the instance, and nothing shows after them.
// Every id is neutral.

use super::gate_369s5::direct_run_package;
use super::instance_workers::{emulator_ladder_fixture, wait_for_ladder_end, wait_until};
use super::*;
use crate::host::ContainedTaskCheckpointPoint;
use actingcommand_contract::{
    EmulatorInstanceAction, InstanceActivity, InstanceActivityKind, RequestId,
    RuntimeInstanceStatus, SchedulingPauseReason, SchedulingPauseScope,
};
use std::net::SocketAddr;
use std::sync::Mutex;

const ALIAS: &str = "r0b.instance.a";
const OTHER: &str = "r0b.instance.b";
/// The neutral direct-run package's `package_id` (`neutral_contained_task_package`).
const PACKAGE_ID: &str = "neutral.semantic.task";
/// The fake clock's wall reading at the start, which no real clock gives.
const CLOCK_UNIX_MS: u64 = 7_000;

/// Every instance row of a `Status` read on `client`, and the rows as the wire carries them.
fn status_rows(client: &mut TestClient) -> (Vec<RuntimeInstanceStatus>, Vec<serde_json::Value>) {
    let request = client.request(RuntimeOperation::Status);
    let receipt = client.send(&request);
    let Some(RuntimeResult::Status { status }) = receipt.result() else {
        panic!("the status: {receipt:?}");
    };
    let wire = serde_json::to_value(status).expect("the status encodes");
    let rows = wire["instances"]
        .as_array()
        .expect("the instance rows")
        .clone();
    (status.instances().to_vec(), rows)
}

/// The activity `alias` shows in a `Status` read on a new connection to `address`.
fn activity_at(address: SocketAddr, alias: &str) -> Option<InstanceActivity> {
    let mut client = TestClient::connect_address(address);
    activity_of(&status_rows(&mut client).0, alias)
}

fn activity_of(rows: &[RuntimeInstanceStatus], alias: &str) -> Option<InstanceActivity> {
    rows.iter()
        .find(|row| row.instance_alias() == alias)
        .unwrap_or_else(|| panic!("a row of {alias}: {rows:?}"))
        .activity()
        .cloned()
}

/// The wire row of `alias` has no `activity` key.
fn assert_no_member(rows: &[serde_json::Value], alias: &str) {
    let row = rows
        .iter()
        .find(|row| row["instance_alias"] == serde_json::json!(alias))
        .unwrap_or_else(|| panic!("a wire row of {alias}: {rows:?}"));
    assert!(row.get("activity").is_none(), "{alias}: {row}");
}

fn wall_ms(clock: &ManualRuntimeClock) -> u64 {
    RuntimeClock::sample(clock).expect("the fake clock").unix_ms
}

fn direct_run(ids: &IdentifierIssuer, alias: &str, package: &str) -> RuntimeRequest {
    runtime_request(
        ids,
        RuntimeOperation::run_contained_task(
            alias,
            ids.mint_holder_id().expect("holder"),
            ContainedTaskRequest::new(package).expect("task request"),
        ),
    )
}

/// What a hook saw at the run's package admission: the activity of `ALIAS` and the fake clock's
/// wall reading then.
type Seen = Arc<Mutex<Option<(Option<InstanceActivity>, u64)>>>;

// Test 1, 2 and 5 of the work order (direct run): while a direct run runs, `ALIAS` shows
// `direct_run` with the client's request, the package's id and the fake clock's reading at its
// grant, and `OTHER`, which does nothing, has no `activity` key. After the run succeeds, fails or
// is cancelled, neither instance has one.
#[test]
fn r0b_a_direct_run_shows_its_request_and_package_and_every_end_clears_it() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let other_state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let a = instance_id();
    let clock = Arc::new(ManualRuntimeClock::new(CLOCK_UNIX_MS, 0));
    let host = RuntimeHost::start(
        config(&root).with_runtime_clock(clock.clone()),
        Arc::new(FakeProvider::from_entries([
            (ALIAS.to_owned(), a, Arc::clone(&state)),
            (OTHER.to_owned(), instance_id(), Arc::clone(&other_state)),
        ])),
    )
    .expect("runtime host with two instances");
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let mut reader = TestClient::connect(&host);
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let (rows, wire) = status_rows(&mut reader);
    assert_eq!(activity_of(&rows, ALIAS), None);
    assert_no_member(&wire, ALIAS);
    assert_no_member(&wire, OTHER);

    // `end` runs in the hook after the status read: nothing (a success), captures that stay
    // unknown (a failure), or a cancel of the run on another connection.
    let cases: [(&str, fn(&FakeState, SocketAddr, RequestId)); 3] = [
        ("a success", |_, _, _| {}),
        ("a failure", |state, _, _| {
            state.unknown_capture.store(true, Ordering::Release);
        }),
        ("a cancel", |_, address, run| {
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
        }),
    ];
    for (connection, (name, end)) in (1381_u64..).zip(cases) {
        // Each run starts on the home page: no input yet, captures known.
        state.input_count.store(0, Ordering::Release);
        state.unknown_capture.store(false, Ordering::Release);
        let request = direct_run(&ids, ALIAS, &package);
        let run = request.request_id();
        let seen: Seen = Arc::new(Mutex::new(None));
        let into = Arc::clone(&seen);
        let hook_state = Arc::clone(&state);
        let hook_clock = Arc::clone(&clock);
        let checkpoint = host
            .run_at_contained_task_checkpoint_for_test(run, a, None, move |_| {
                *into.lock().expect("seen") =
                    Some((activity_at(address, ALIAS), wall_ms(&hook_clock)));
                end(&hook_state, address, run);
            })
            .expect("arm the run's status read");
        let started_at = wall_ms(&clock);

        let receipt = host
            .process_request_for_test(&request, ConnectionId::new(connection).expect("connection"))
            .expect("the run's receipt");

        assert_eq!(checkpoint.consumed(), 1, "{name}: the run was admitted");
        let (during, read_at) = seen.lock().expect("seen").take().expect("the status read");
        let during = during.unwrap_or_else(|| panic!("{name}: the run shows"));
        assert_eq!(during.kind, InstanceActivityKind::DirectRun, "{name}");
        assert_eq!(during.request_id, Some(run), "{name}");
        assert_eq!(during.package_id.as_deref(), Some(PACKAGE_ID), "{name}");
        assert!(
            (started_at..=read_at).contains(&during.since_unix_ms),
            "{name}: since {} from the fake clock, {started_at}..={read_at}",
            during.since_unix_ms
        );
        match name {
            "a success" => assert!(
                matches!(
                    receipt.result(),
                    Some(RuntimeResult::ContainedTaskCompleted {
                        task_result: TaskOutcome::Success,
                        ..
                    })
                ),
                "{receipt:?}"
            ),
            "a failure" => assert!(
                !matches!(
                    receipt.result(),
                    Some(
                        RuntimeResult::ContainedTaskCompleted {
                            task_result: TaskOutcome::Success,
                            ..
                        } | RuntimeResult::ContainedTaskCancelled { .. }
                    )
                ),
                "{receipt:?}"
            ),
            _ => assert!(
                matches!(
                    receipt.result(),
                    Some(RuntimeResult::ContainedTaskCancelled { .. })
                ),
                "{receipt:?}"
            ),
        }
        let (rows, wire) = status_rows(&mut reader);
        assert_eq!(activity_of(&rows, ALIAS), None, "after {name}");
        assert_no_member(&wire, ALIAS);
        assert_no_member(&wire, OTHER);
        assert!(
            host.instance_activities_for_test(ALIAS)
                .expect("the activities")
                .is_empty(),
            "after {name}"
        );
    }
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(reader);
    host.close().expect("close host");
}

// Test 1 and 3 of the work order (policy run, ladder, overlap): the policy run shows
// `policy_run` with the request it runs as (the one its checkpoint names) and a package. It fails
// and hands its key to its ladder; while the ladder's emulator restart drives, two activities are
// registered, the ladder on its key and the restart on the ladder's, and the status shows the
// ladder. After the ladder released its key, nothing shows and nothing is registered.
#[test]
fn r0b_a_policy_run_shows_then_its_ladder_shows_over_the_restart_on_its_key() {
    let run = emulator_ladder_fixture(0, false);
    let address = run
        .host
        .runtime_info()
        .socket_addr()
        .expect("runtime address");
    let lease = run.context.lease_token().clone();
    let seen = Arc::new(Mutex::new(None));
    let into = Arc::clone(&seen);
    let checkpoint = run
        .host
        .run_at_leased_checkpoint_for_test(
            ContainedTaskCheckpointPoint::PackageAdmitted,
            lease.instance_id(),
            lease.lease_id(),
            move |identity| {
                *into.lock().expect("seen") = Some((
                    identity.request_id(),
                    activity_at(address, POLICY_INSTANCE_ALIAS),
                ));
            },
        )
        .expect("arm the policy run's status read");
    run.state
        .block_emulator_start
        .store(true, Ordering::Release);
    run.state.fail_emulator_start.store(true, Ordering::Release);

    run.fail_scheduled_run();

    assert_eq!(checkpoint.consumed(), 1, "the policy run was admitted");
    let (request_id, during) = seen.lock().expect("seen").take().expect("the status read");
    let during = during.expect("the policy run shows");
    assert_eq!(during.kind, InstanceActivityKind::PolicyRun);
    assert_eq!(during.request_id, Some(request_id));
    // The fixture's scheduled package is the neutral one (`ladder_setup`).
    assert_eq!(during.package_id.as_deref(), Some(PACKAGE_ID), "{during:?}");

    wait_until("rung 3's Start", || {
        run.state.emulator_start_entered.load(Ordering::Acquire)
    });
    assert_eq!(
        run.host
            .instance_activities_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the activities"),
        [
            (InstanceActivityKind::Ladder, true),
            (InstanceActivityKind::EmulatorControl, false),
        ],
        "the ladder on its key, its restart on the ladder's"
    );
    let shown = activity_at(address, POLICY_INSTANCE_ALIAS).expect("the ladder shows");
    assert_eq!(shown.kind, InstanceActivityKind::Ladder);
    assert!(shown.request_id.is_some() && shown.package_id.is_none());

    run.state
        .block_emulator_start
        .store(false, Ordering::Release);
    wait_for_ladder_end(&run.host, POLICY_INSTANCE_ALIAS);
    assert_eq!(activity_at(address, POLICY_INSTANCE_ALIAS), None);
    assert!(
        run.host
            .instance_activities_for_test(POLICY_INSTANCE_ALIAS)
            .expect("the activities")
            .is_empty()
    );
    assert!(run.host.fatal_error().expect("runtime health").is_none());
    run.host.close().expect("close host");
}

// Test 1 and 2 of the work order (self-check, reconnect, emulator control): after a direct run
// kept the instance's session, a self-check closes it first, and after another one a resume's
// reconnect does: at each close the status shows `selfcheck` with the self-check's request, then
// `reconnect` with the resume's. An operator's Start shows `emulator_control` with its request
// while it drives. After each, nothing shows.
#[test]
fn r0b_a_self_check_a_reconnect_and_an_emulator_control_show_while_they_hold_the_key() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let package = direct_run_package(&root, &state);
    let a = instance_id();
    let host = RuntimeHost::start(
        config(&root),
        Arc::new(FakeProvider::one(ALIAS, a, Arc::clone(&state))),
    )
    .expect("runtime host");
    let address = host.runtime_info().socket_addr().expect("runtime address");
    let mut client = TestClient::connect(&host);
    client.set_receipt_read_timeout();
    let ids = IdentifierIssuer::new().expect("identifier issuer");

    // A direct run keeps the instance's session (Workflow #191 H).
    let kept = client.send(&direct_run(&ids, ALIAS, &package));
    assert_eq!(kept.state(), RuntimeReceiptState::Completed, "{kept:?}");

    // The self-check.
    let seen = Arc::new(Mutex::new(None));
    let into = Arc::clone(&seen);
    *state.input_close_hook.lock().expect("input close hook") = Some(Box::new(move || {
        *into.lock().expect("seen") = Some(activity_at(address, ALIAS));
    }));
    let check = client.request(RuntimeOperation::SelfCheckInstance {
        instance_alias: ALIAS.to_owned(),
    });
    let checked = client.send(&check);
    assert_eq!(
        checked.state(),
        RuntimeReceiptState::Completed,
        "{checked:?}"
    );
    let during = seen
        .lock()
        .expect("seen")
        .take()
        .expect("the self-check closed the kept session")
        .expect("the self-check shows");
    assert_eq!(during.kind, InstanceActivityKind::Selfcheck);
    assert_eq!(during.request_id, Some(check.request_id()));
    assert_eq!(during.package_id, None);
    assert_eq!(activity_at(address, ALIAS), None, "after the self-check");

    // The resume's reconnect.
    let scope = SchedulingPauseScope::Instance {
        instance_alias: ALIAS.to_owned(),
    };
    let pause = client.request(RuntimeOperation::PauseScheduling {
        scope: scope.clone(),
        pause_reason: SchedulingPauseReason::new("gate_check").expect("pause reason"),
        drain_timeout_ms: 1_000,
    });
    assert_eq!(client.send(&pause).state(), RuntimeReceiptState::Completed);
    // A pause does not hold a direct run back; this one keeps a session for the reconnect. It
    // starts on the home page again (no input yet).
    state.input_count.store(0, Ordering::Release);
    let kept = client.send(&direct_run(&ids, ALIAS, &package));
    assert_eq!(kept.state(), RuntimeReceiptState::Completed, "{kept:?}");
    let into = Arc::clone(&seen);
    *state.input_close_hook.lock().expect("input close hook") = Some(Box::new(move || {
        *into.lock().expect("seen") = Some(activity_at(address, ALIAS));
    }));
    let resume = client.request(RuntimeOperation::ResumeScheduling {
        scope,
        expected: None,
    });
    let resumed = client.send(&resume);
    assert_eq!(
        resumed.state(),
        RuntimeReceiptState::Completed,
        "{resumed:?}"
    );
    let during = seen
        .lock()
        .expect("seen")
        .take()
        .expect("the reconnect closed the kept session")
        .expect("the reconnect shows");
    assert_eq!(during.kind, InstanceActivityKind::Reconnect);
    assert_eq!(during.request_id, Some(resume.request_id()));
    assert_eq!(during.package_id, None);
    let mut reader = TestClient::connect(&host);
    let (rows, wire) = status_rows(&mut reader);
    assert_eq!(activity_of(&rows, ALIAS), None, "after the reconnect");
    assert_no_member(&wire, ALIAS);

    // The emulator control.
    bind_fake_emulator(&host, ALIAS, a, &state);
    state.block_emulator_start.store(true, Ordering::Release);
    let control = runtime_request(
        &ids,
        RuntimeOperation::ControlEmulatorInstance {
            instance_alias: ALIAS.to_owned(),
            action: EmulatorInstanceAction::Start,
        },
    );
    thread::scope(|scope| {
        let driving = scope.spawn(|| {
            host.process_request_for_test(&control, ConnectionId::new(1391).expect("connection"))
        });
        wait_until("the control's Start", || {
            state.emulator_start_entered.load(Ordering::Acquire)
        });
        let shown = activity_at(address, ALIAS).expect("the control shows");
        assert_eq!(shown.kind, InstanceActivityKind::EmulatorControl);
        assert_eq!(shown.request_id, Some(control.request_id()));
        assert_eq!(shown.package_id, None);
        state.block_emulator_start.store(false, Ordering::Release);
        driving
            .join()
            .expect("the control's thread")
            .expect("the control's receipt");
    });
    let (rows, wire) = status_rows(&mut reader);
    assert_eq!(activity_of(&rows, ALIAS), None, "after the control");
    assert_no_member(&wire, ALIAS);
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(reader);
    drop(client);
    host.close().expect("close host");
}
