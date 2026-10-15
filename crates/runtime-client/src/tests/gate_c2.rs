// SPDX-License-Identifier: AGPL-3.0-only

// #382 C-2 (test plan v2.2 C-2), each over `ScriptedRuntime` (inventory-contract I1).
// - G4, inventory key R4 (with R3): an installation request whose receipt never came is
//   unconfirmed, uncertain and sent once; the client never retries it (Workflow #352,
//   INSTALL.md "A missing client receipt is unknown ... The client never retries"). A receipt
//   for another transition or ticket is unexpected; CommitShutdown takes its ShutdownAccepted
//   and Release its own ticket.
// - G5, inventory key R2: the latch table (Workflow #376 M2: a non-fatal rejection does not
//   latch, only fatal errors do; input-receipt-wait.md: a timeout or a disconnected transport
//   stays latched and authorizes no replay; client-interactions.md: a latched connection is not
//   reopened; Workflow #338 R6: an owner epoch that changed in between is uncertain), and the
//   input whose receipt never came (test plan §4 #7's replacement, input-receipt-wait.md
//   "Receipt-header I/O context").
// - G8, inventory key R12: emulator control (emulator-control.md "Host codes"; Workflow #338
//   R6: `backend_operation_failed` is a device failure).
// - G9, inventory key R7: the run status of one contained run (Workflow #338 R1 and its
//   ruling 5979163725).
// - Test plan §4 #6's replacement, inventory key R5: a contained task's receipt outlives the
//   IO budget (contracts/README.md: the task receipt waits for the task's deadline).

use super::{RejectProjectionSecrets, contained_task_request};
use crate::test_support::{ScriptedAnswer, ScriptedConnection, ScriptedGate, ScriptedRuntime};
use crate::{
    ClientCode, ContainedRunLease, ContainedRunProgress, ContainedRunState, ContainedRunStatus,
    RunDispatch, RunKey, RunOrigin, RunStatusMode, RuntimeClient, RuntimeClientConfig,
    RuntimeClientError, RuntimeClientErrorClass, RuntimeClientResult,
};
use actingcommand_contract::codes::{ContractLocation, SharedCode};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{
    AuditInput, ClientPayloadDraft, CommandPayloadDraft, ContainedTaskCancellationReason,
    ContainedTaskLeaseTerminal, ContainedTaskRequest, EffectDisposition, EmulatorInstanceAction,
    EventAction, EventActor, EventDraft, EventLinksDraft, EventOrigin, EventPayloadDraft,
    EventSeverity, EventSource, IdentifierIssuer, InputAction, InstallTransitionAction,
    InstallTransitionPhase, InstallTransitionStatus, InstallTransitionTicket, LeaseId,
    LeasePayloadDraft, LeaseToken, OriginModule, OwnerEpoch, ProjectedEvent, ProjectionPayload,
    RequestId, RunId, RuntimeErrorCode, RuntimeErrorProjection, RuntimeOperation,
    RuntimePayloadDraft, RuntimeReceiptState, RuntimeResult, StartupPackageDisposition,
    TaskOutcome, TaskPayloadDraft, TaskSemanticFact,
};
use actingcommand_runtime_host::HostCode;
use std::path::Path;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// An IO budget a receipt the script holds back outlasts.
const SHORT_IO: Duration = Duration::from_millis(250);
/// An IO budget no answered exchange of these tests comes near.
const IO: Duration = Duration::from_secs(10);
/// The input test's budgets, at the 5:12 ratio input-receipt-wait.md keeps: a tap waits
/// B + G + B + I, here 1 450 ms.
const INPUT_IO: Duration = SHORT_IO;
const INPUT_BACKEND_OPEN: Duration = Duration::from_millis(600);
const INSTANCE: &str = "node-01";
const OTHER_INSTANCE: &str = "node-02";
const TRANSITION: &str = "transition-0001";
const OTHER_TRANSITION: &str = "transition-0002";
const INSTALL_TIMEOUT_MS: u64 = 30_000;

fn connect(state_root: &Path, io_timeout: Duration) -> RuntimeClient {
    RuntimeClient::connect(
        RuntimeClientConfig::new(
            state_root,
            crate::RuntimeChannel::Cli,
            EventActor::Cli,
            EventSource::Cli,
        )
        .with_io_timeout(io_timeout),
    )
    .expect("runtime client")
}

fn assert_code(error: &RuntimeClientError, code: Code, row: &str) {
    assert_eq!(error.code().as_str(), code.as_str(), "{row}: {error:?}");
}

/// The installation status of `ticket` in `phase`, as the Runtime answers a transition.
fn install_status(ticket: InstallTransitionTicket, phase: InstallTransitionPhase) -> RuntimeResult {
    RuntimeResult::InstallTransition {
        status: InstallTransitionStatus {
            ticket,
            phase,
            admission_closed: !matches!(
                phase,
                InstallTransitionPhase::Released | InstallTransitionPhase::Aborted
            ),
            timeout_ms: INSTALL_TIMEOUT_MS,
            failure_code: None,
        },
    }
}

/// A ticket of the scripted Runtime's own installation transition `transition_id`.
fn ticket(client: &RuntimeClient, transition_id: &str) -> InstallTransitionTicket {
    let issuer = IdentifierIssuer::new().expect("identifier issuer");
    InstallTransitionTicket {
        target: client.runtime_info().shutdown_target(),
        transition_id: transition_id.to_owned(),
        request_id: *issuer.mint_request_id().expect("request id").transport(),
    }
}

// G4 (R4, R3): a Query whose receipt never came, past the IO budget, is
// `install_transition_receipt_unconfirmed` and uncertain, and the script saw exactly one
// installation request: no retry, no reconnect. The script holds the request until the client
// closes its connection.
#[test]
fn gate_an_install_transition_whose_receipt_never_came_is_unconfirmed_uncertain_and_sent_once() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            assert!(
                matches!(
                    request.operation(),
                    RuntimeOperation::InstallTransition {
                        action: InstallTransitionAction::Query { .. },
                        ..
                    }
                ),
                "{:?}",
                request.operation()
            );
            ScriptedAnswer::StallUntilClosed
        })]
    });
    let client = connect(root.path(), SHORT_IO);
    let error = client
        .install_transition(InstallTransitionAction::Query {
            transition_id: TRANSITION.to_owned(),
        })
        .expect_err("a Query whose receipt never came");
    drop(client);
    let record = runtime.finish();

    assert_code(
        &error,
        ClientCode::InstallTransitionReceiptUnconfirmed.code(),
        "unconfirmed",
    );
    assert!(error.projection().is_none(), "{error:?}");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
    assert_eq!(
        record.connections[0]
            .count(|operation| matches!(operation, RuntimeOperation::InstallTransition { .. })),
        1,
        "installation requests the script saw"
    );
    assert!(
        record.unscripted.is_empty(),
        "the client reconnected: {:?}",
        record.unscripted
    );
}

// G4 (R4): a receipt that answers another transition or another ticket than the one asked for
// is `runtime_result_unexpected`, an uncertain failure after send.
#[test]
fn gate_an_install_transition_receipt_for_another_transition_or_ticket_is_unexpected() {
    // Query: the receipt names another transition.
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            let RuntimeOperation::InstallTransition { target, .. } = request.operation() else {
                panic!("an installation request, got {:?}", request.operation())
            };
            let other = InstallTransitionTicket {
                target: *target,
                transition_id: OTHER_TRANSITION.to_owned(),
                request_id: request.request_id(),
            };
            ScriptedAnswer::completed(
                request,
                install_status(other, InstallTransitionPhase::Draining),
            )
        })]
    });
    let client = connect(root.path(), IO);
    let query = client.install_transition(InstallTransitionAction::Query {
        transition_id: TRANSITION.to_owned(),
    });
    drop(client);
    runtime.finish();
    let error = query.expect_err("a Query answered for another transition");
    assert_code(&error, ClientCode::RuntimeResultUnexpected.code(), "query");
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);

    // Release: the receipt names another ticket.
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            let RuntimeOperation::InstallTransition {
                action: InstallTransitionAction::Release { ticket, .. },
                ..
            } = request.operation()
            else {
                panic!("a Release, got {:?}", request.operation())
            };
            let other = InstallTransitionTicket {
                transition_id: OTHER_TRANSITION.to_owned(),
                ..ticket.clone()
            };
            ScriptedAnswer::completed(
                request,
                install_status(other, InstallTransitionPhase::Released),
            )
        })]
    });
    let client = connect(root.path(), IO);
    let release = client.install_transition(InstallTransitionAction::Release {
        ticket: ticket(&client, TRANSITION),
        timeout_ms: INSTALL_TIMEOUT_MS,
    });
    drop(client);
    runtime.finish();
    let error = release.expect_err("a Release answered for another ticket");
    assert_code(
        &error,
        ClientCode::RuntimeResultUnexpected.code(),
        "release",
    );
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Uncertain);
}

// G4 (R4): CommitShutdown answered with the shutdown of this Runtime is Ok, and so is a
// Release answered for its own ticket.
#[test]
fn gate_commit_shutdown_takes_its_shutdown_and_release_its_own_ticket() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![
            ScriptedConnection::new()
                .request(|request| {
                    let RuntimeOperation::InstallTransition {
                        target,
                        action: InstallTransitionAction::CommitShutdown { .. },
                    } = request.operation()
                    else {
                        panic!("a CommitShutdown, got {:?}", request.operation())
                    };
                    let target = *target;
                    ScriptedAnswer::completed(request, RuntimeResult::ShutdownAccepted { target })
                })
                .request(|request| {
                    let RuntimeOperation::InstallTransition {
                        action: InstallTransitionAction::Release { ticket, .. },
                        ..
                    } = request.operation()
                    else {
                        panic!("a Release, got {:?}", request.operation())
                    };
                    let ticket = ticket.clone();
                    ScriptedAnswer::completed(
                        request,
                        install_status(ticket, InstallTransitionPhase::Released),
                    )
                }),
        ]
    });
    let client = connect(root.path(), IO);
    let commit = client.install_transition(InstallTransitionAction::CommitShutdown {
        ticket: ticket(&client, TRANSITION),
    });
    let release = client.install_transition(InstallTransitionAction::Release {
        ticket: ticket(&client, OTHER_TRANSITION),
        timeout_ms: INSTALL_TIMEOUT_MS,
    });
    drop(client);
    runtime.finish();
    let commit = commit.expect("CommitShutdown answered with this Runtime's shutdown");
    assert!(matches!(
        commit.result(),
        Some(RuntimeResult::ShutdownAccepted { .. })
    ));
    let release = release.expect("Release answered for its own ticket");
    assert!(matches!(
        release.result(),
        Some(RuntimeResult::InstallTransition { .. })
    ));
}

/// One row of the latch table.
struct LatchRow {
    row: &'static str,
    io_timeout: Duration,
    /// The script of the client's connections, given the Runtime's owner epoch.
    connections: fn(OwnerEpoch) -> Vec<ScriptedConnection>,
    /// The call that fails.
    call: fn(&RuntimeClient) -> RuntimeClientResult<()>,
    /// The failure's own code and class.
    code: Code,
    class: RuntimeClientErrorClass,
    /// Latched: the code the next call fails with, without reaching the script. Not latched
    /// (`None`): the next call reaches the script and is answered.
    latched: Option<Code>,
    /// The request frames each scripted connection receives, the handshake's included.
    frames: &'static [usize],
}

fn acquire(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client.acquire_lease(INSTANCE).map(drop)
}

fn health(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client.health().map(drop)
}

fn resource_target_view(client: &RuntimeClient) -> RuntimeClientResult<()> {
    client.resource_target_view(INSTANCE).map(drop)
}

/// The script's answer to an acquisition: a refusal with `code`, fatal or not.
fn refuse_acquisition(code: RuntimeErrorCode, fatal: bool) -> ScriptedConnection {
    ScriptedConnection::new().request(move |request| {
        assert!(
            matches!(request.operation(), RuntimeOperation::AcquireLease { .. }),
            "{:?}",
            request.operation()
        );
        let projection = RuntimeErrorProjection::new(code, fatal);
        let state = if fatal {
            RuntimeReceiptState::Failed
        } else {
            RuntimeReceiptState::Denied
        };
        ScriptedAnswer::failed(request, state, projection.receipt_outcome(None, None))
    })
}

fn expect_health(request: &actingcommand_contract::RuntimeRequest) {
    assert!(
        matches!(request.operation(), RuntimeOperation::Health),
        "{:?}",
        request.operation()
    );
}

fn latch_rows() -> [LatchRow; 5] {
    [
        LatchRow {
            row: "non-fatal rejection",
            io_timeout: IO,
            connections: |owner_epoch| {
                vec![
                    refuse_acquisition(RuntimeErrorCode::LeaseBusy, false).request(
                        move |request| {
                            expect_health(request);
                            ScriptedAnswer::completed(
                                request,
                                RuntimeResult::Health { owner_epoch },
                            )
                        },
                    ),
                ]
            },
            call: acquire,
            code: ClientCode::RuntimeRequestRejected.code(),
            class: RuntimeClientErrorClass::Safety,
            latched: None,
            frames: &[4],
        },
        LatchRow {
            row: "fatal rejection",
            io_timeout: IO,
            connections: |_| vec![refuse_acquisition(RuntimeErrorCode::RuntimeHalted, true)],
            call: acquire,
            code: ClientCode::RuntimeRequestRejected.code(),
            class: RuntimeClientErrorClass::Runtime,
            latched: Some(ClientCode::RuntimeRequestRejected.code()),
            frames: &[3],
        },
        LatchRow {
            row: "receipt header timeout",
            io_timeout: SHORT_IO,
            connections: |_| {
                vec![ScriptedConnection::new().request(|request| {
                    expect_health(request);
                    ScriptedAnswer::StallUntilClosed
                })]
            },
            call: health,
            code: ClientCode::RuntimeReceiptHeaderFailed.code(),
            class: RuntimeClientErrorClass::Uncertain,
            latched: Some(ClientCode::RuntimeReceiptHeaderFailed.code()),
            frames: &[3],
        },
        LatchRow {
            row: "end of stream before the deadline",
            io_timeout: IO,
            connections: |_| {
                vec![ScriptedConnection::new().request(|request| {
                    expect_health(request);
                    ScriptedAnswer::Close
                })]
            },
            call: health,
            code: ClientCode::RuntimeReceiptHeaderFailed.code(),
            class: RuntimeClientErrorClass::Uncertain,
            latched: Some(ClientCode::RuntimeReceiptHeaderFailed.code()),
            frames: &[3],
        },
        LatchRow {
            // The connection ends before the receipt of a request a Runtime may not know, the
            // client reconnects in the same owner epoch (`send_or_dropped`), and the address
            // now answers another owner epoch.
            row: "another owner epoch on the reconnect",
            io_timeout: IO,
            connections: |_| {
                let other = *IdentifierIssuer::new()
                    .expect("identifier issuer")
                    .mint_owner_epoch()
                    .expect("another owner epoch")
                    .transport();
                vec![
                    ScriptedConnection::new().request(|request| {
                        assert!(
                            matches!(
                                request.operation(),
                                RuntimeOperation::ResourceTargetView { .. }
                            ),
                            "{:?}",
                            request.operation()
                        );
                        ScriptedAnswer::Close
                    }),
                    ScriptedConnection::in_other_epoch(other),
                ]
            },
            call: resource_target_view,
            code: ClientCode::RuntimeReceiptHeaderFailed.code(),
            class: RuntimeClientErrorClass::Uncertain,
            latched: Some(ClientCode::RuntimeOwnerEpochChanged.code()),
            frames: &[3, 1],
        },
    ]
}

// G5 (R2): the latch table. Each row: whether the failure latches the client, whether the
// next call reaches the script, and the codes and classes.
#[test]
fn gate_a_client_latches_on_fatal_and_transport_failures_and_never_on_a_non_fatal_rejection() {
    for row in latch_rows() {
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), row.connections);
        let client = connect(root.path(), row.io_timeout);
        let first = (row.call)(&client).expect_err(row.row);
        let next = client.health();
        drop(client);
        let record = runtime.finish();

        assert_code(&first, row.code, row.row);
        assert_eq!(first.disposition(), row.class, "{}: {first:?}", row.row);
        match row.latched {
            None => {
                assert!(!first.is_fatal(), "{}: {first:?}", row.row);
                next.unwrap_or_else(|error| {
                    panic!("{}: the next call reaches the script: {error:?}", row.row)
                });
            }
            Some(code) => {
                let next = next.expect_err(row.row);
                assert_code(&next, code, row.row);
                assert_eq!(next.disposition(), row.class, "{}: {next:?}", row.row);
                if code.as_str() == first.code().as_str() {
                    assert_eq!(
                        next, first,
                        "{}: the latched error is the original",
                        row.row
                    );
                } else {
                    // The failure that latched later in the same call is linked to it.
                    assert!(
                        first
                            .outcome()
                            .causes()
                            .iter()
                            .any(|link| link.outcome().code().as_str() == code.as_str()),
                        "{}: {} is not linked to {first:?}",
                        row.row,
                        code.as_str()
                    );
                }
            }
        }
        assert_eq!(
            record
                .connections
                .iter()
                .map(|connection| connection.requests.len())
                .collect::<Vec<_>>(),
            row.frames,
            "{}: the request frames each connection received",
            row.row
        );
        assert!(
            record.unscripted.is_empty(),
            "{}: the client reconnected: {:?}",
            row.row,
            record.unscripted
        );
    }
}

// G5 (R2), test plan §4 #7's replacement: an input whose receipt never came, past its finite
// wait (B + G + B + I), latches the client with the original request's context on the
// receipt-header read (request and correlation, the Debug session's correlation for a Debug
// input, the expected owner epoch and PID, the I/O kind and at most 256 characters of its
// text). The input is sent exactly once: the next input and the release fail with the same
// error, and the script sees neither.
#[test]
fn gate_an_input_whose_receipt_never_came_latches_with_its_request_context_and_is_sent_once() {
    for debug in [false, true] {
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), |owner_epoch| {
            vec![
                ScriptedConnection::new()
                    .request(move |request| {
                        let RuntimeOperation::AcquireLease { holder_id, .. } = request.operation()
                        else {
                            panic!("an acquisition, got {:?}", request.operation())
                        };
                        let issuer = IdentifierIssuer::new().expect("identifier issuer");
                        let token = LeaseToken::new(
                            owner_epoch,
                            *issuer.mint_lease_id().expect("lease id").transport(),
                            *issuer.mint_instance_id().expect("instance id").transport(),
                            *holder_id,
                            60_000,
                        )
                        .expect("lease token");
                        ScriptedAnswer::answered(
                            request,
                            RuntimeReceiptState::Admitted,
                            RuntimeResult::LeaseGranted { token },
                        )
                    })
                    .request(|request| {
                        assert!(
                            matches!(request.operation(), RuntimeOperation::Input { .. }),
                            "{:?}",
                            request.operation()
                        );
                        ScriptedAnswer::StallUntilClosed
                    }),
            ]
        });
        let client = RuntimeClient::connect(
            RuntimeClientConfig::new(
                root.path(),
                crate::RuntimeChannel::Lab,
                EventActor::Lab,
                EventSource::Lab,
            )
            .with_io_timeout(INPUT_IO)
            .with_backend_open_timeout(INPUT_BACKEND_OPEN),
        )
        .expect("Lab runtime client");
        let info = client.runtime_info().clone();
        let session = debug.then(|| client.begin_debug_session().expect("debug session"));
        let session_correlation = session.as_ref().map(|session| session.correlation_id());
        let token = match &session {
            Some(session) => session.acquire_lease(INSTANCE),
            None => client.acquire_lease(INSTANCE),
        }
        .expect("lease");
        let input = || match &session {
            Some(session) => session.input(&token, InputAction::Tap { x: 10, y: 20 }),
            None => client.input(&token, InputAction::Tap { x: 10, y: 20 }),
        };
        let error = input().expect_err("an input whose receipt never came");
        let again = input().expect_err("an input on a latched client");
        let release = client
            .release_lease(&token)
            .expect_err("a release on a latched client");
        drop(session);
        drop(client);
        let record = runtime.finish();

        let row = if debug { "debug input" } else { "input" };
        let inputs = record.connections[0]
            .requests
            .iter()
            .filter(|request| matches!(request.operation(), RuntimeOperation::Input { .. }))
            .collect::<Vec<_>>();
        let [sent] = inputs.as_slice() else {
            panic!("{row}: the script saw {} inputs", inputs.len())
        };
        assert_code(&error, ClientCode::RuntimeReceiptHeaderFailed.code(), row);
        assert!(error.is_fatal(), "{row}: {error:?}");
        assert!(error.committed_receipt().is_none(), "{row}: {error:?}");
        let io = error
            .receipt_header_io()
            .unwrap_or_else(|| panic!("{row}: the receipt-header I/O context: {error:?}"));
        assert!(
            matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ),
            "{row}: {io:?}"
        );
        assert_eq!(io.request_id(), Some(&sent.request_id()), "{row}");
        assert_eq!(io.correlation_id(), Some(&sent.correlation_id()), "{row}");
        if let Some(correlation) = session_correlation {
            assert_eq!(sent.correlation_id(), correlation, "{row}");
        }
        assert_eq!(
            io.expected_owner_epoch(),
            Some(&info.owner_epoch()),
            "{row}"
        );
        assert_eq!(io.expected_runtime_pid(), Some(info.pid()), "{row}");
        assert!(!io.message().is_empty(), "{row}");
        assert!(io.message().chars().count() <= 256, "{row}");
        assert_eq!(again, error, "{row}: the next input");
        assert_eq!(release, error, "{row}: the release");
        assert_eq!(
            record.connections[0]
                .count(|operation| matches!(operation, RuntimeOperation::ReleaseLease { .. })),
            0,
            "{row}: a release reached the script"
        );
        assert!(
            record.unscripted.is_empty(),
            "{row}: the client reconnected: {:?}",
            record.unscripted
        );
    }
}

// G8 (R12): a restart the host's wait timed out on comes back as the host's failure: the
// typed rejection carries the host code `emulator_control_wait_timeout` with its operation
// (receipt v2, Workflow #378 A4), and its class is device (`backend_operation_failed`), not
// uncertain.
#[test]
fn gate_a_restart_the_host_timed_out_is_a_device_failure_carrying_its_host_code() {
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), |_| {
        vec![ScriptedConnection::new().request(|request| {
            assert!(
                matches!(
                    request.operation(),
                    RuntimeOperation::ControlEmulatorInstance {
                        action: EmulatorInstanceAction::Restart,
                        ..
                    }
                ),
                "{:?}",
                request.operation()
            );
            let projection =
                RuntimeErrorProjection::new(RuntimeErrorCode::BackendOperationFailed, false);
            let outcome = projection.receipt_outcome(
                Some(HostCode::EmulatorControlWaitTimeout.code()),
                Some(ContractLocation::ControlEmulatorInstance.location()),
            );
            ScriptedAnswer::failed(request, RuntimeReceiptState::Failed, outcome)
        })]
    });
    let client = connect(root.path(), IO);
    let restart = client.control_emulator_instance(INSTANCE, EmulatorInstanceAction::Restart);
    drop(client);
    runtime.finish();

    let error = restart.expect_err("a restart the host timed out");
    assert_eq!(
        error.host_failure(),
        Some((
            HostCode::EmulatorControlWaitTimeout.as_str(),
            ContractLocation::ControlEmulatorInstance.as_str()
        ))
    );
    assert_eq!(
        error.projection().map(|projection| projection.code),
        Some(RuntimeErrorCode::BackendOperationFailed)
    );
    assert_eq!(error.disposition(), RuntimeClientErrorClass::Device);
    assert!(
        error
            .received_receipt()
            .is_some_and(|receipt| receipt.state() == RuntimeReceiptState::Failed),
        "{error:?}"
    );
}

// G8 (R12): a result that echoes another instance or another action than the one asked for is
// `runtime_result_unexpected`, an uncertain failure after send.
#[test]
fn gate_an_emulator_control_result_for_another_instance_or_action_is_unexpected() {
    for (row, alias, action) in [
        (
            "another instance",
            OTHER_INSTANCE,
            EmulatorInstanceAction::Restart,
        ),
        ("another action", INSTANCE, EmulatorInstanceAction::Start),
    ] {
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), |_| {
            vec![ScriptedConnection::new().request(move |request| {
                assert!(
                    matches!(
                        request.operation(),
                        RuntimeOperation::ControlEmulatorInstance { .. }
                    ),
                    "{:?}",
                    request.operation()
                );
                ScriptedAnswer::completed(
                    request,
                    RuntimeResult::EmulatorInstanceControlled {
                        instance_alias: alias.to_owned(),
                        action,
                        instance_index: 1,
                        running: true,
                        adb_port: None,
                        elapsed_ms: 1,
                        startup_package: StartupPackageDisposition::None,
                    },
                )
            })]
        });
        let client = connect(root.path(), IO);
        let restart = client.control_emulator_instance(INSTANCE, EmulatorInstanceAction::Restart);
        drop(client);
        runtime.finish();

        let error = restart.expect_err(row);
        assert_code(&error, ClientCode::RuntimeResultUnexpected.code(), row);
        assert_eq!(
            error.disposition(),
            RuntimeClientErrorClass::Uncertain,
            "{row}"
        );
    }
}

// Test plan §4 #6's replacement (R5): a contained task's receipt is accepted after the IO
// budget ran out, and the request carries the default task deadline. Gates order it instead of
// a sleep: the script answers the task only once a second client on the same budget, which
// connected after the task was sent, has had its own receipt wait run out.
#[test]
fn gate_a_contained_task_receipt_outlives_the_io_budget() {
    let task_sent = ScriptedGate::new();
    let budget_spent = ScriptedGate::new();
    let root = TempDir::new().expect("tempdir");
    let runtime = ScriptedRuntime::start(root.path(), {
        let task_sent = task_sent.clone();
        let budget_spent = budget_spent.clone();
        move |_| {
            let answer_after = budget_spent.clone();
            vec![
                // The task's connection.
                ScriptedConnection::new()
                    .request(move |request| {
                        let RuntimeOperation::RunContainedTask { request: task, .. } =
                            request.operation()
                        else {
                            panic!("a contained task, got {:?}", request.operation())
                        };
                        assert_eq!(
                            task.response_deadline_ms(),
                            ContainedTaskRequest::DEFAULT_RESPONSE_DEADLINE_MS
                        );
                        let issuer = IdentifierIssuer::new().expect("identifier issuer");
                        let result = RuntimeResult::ContainedTaskCompleted {
                            run_id: *issuer.mint_run_id().expect("run id").transport(),
                            task_id: *issuer.mint_task_id().expect("task id").transport(),
                            task_request_id: request.request_id(),
                            response_deadline_monotonic_ms: Some(60_000),
                            task_result: TaskOutcome::Success,
                            final_page: None,
                            executed_steps: 1,
                        };
                        task_sent.open();
                        ScriptedAnswer::completed(request, result).after(&answer_after)
                    })
                    // The flow's projection query.
                    .ledger(Vec::new()),
                // The second client's connection: one request whose receipt never comes.
                ScriptedConnection::new()
                    .request(|request| {
                        expect_health(request);
                        ScriptedAnswer::StallUntilClosed
                    })
                    .open(&budget_spent),
            ]
        }
    });
    let second = {
        let state_root = root.path().to_path_buf();
        thread::spawn(move || {
            task_sent.wait();
            let second = connect(&state_root, SHORT_IO);
            second.health()
        })
    };
    let client = connect(root.path(), SHORT_IO);
    let output = client.run_contained_task(INSTANCE, contained_task_request());
    let second = second.join().expect("the second client");
    drop(client);
    let record = runtime.finish();

    let second = second.expect_err("the second client's receipt never comes");
    assert_code(
        &second,
        ClientCode::RuntimeReceiptHeaderFailed.code(),
        "the IO budget ran out",
    );
    let output = output.expect("the task's receipt after the IO budget");
    assert!(matches!(
        output.receipt().result(),
        Some(RuntimeResult::ContainedTaskCompleted { .. })
    ));
    assert_eq!(
        record.connections[0]
            .count(|operation| matches!(operation, RuntimeOperation::RunContainedTask { .. })),
        1
    );
}

/// The fixture run's neutral labels.
const PACKAGE: &str = "neutral.main";
const TASK_LABEL: &str = "neutral.task";
const PAGE: &str = "neutral/home";
const STEP: &str = "neutral.tap";
/// The time of the fixture ledger's first record.
const RECORDED_FROM_UNIX_MS: u64 = 1_800_000_000_000;

/// One contained run's scripted ledger, built record by record in sequence order.
struct RunLedger {
    issuer: IdentifierIssuer,
    request_id: RequestId,
    run_id: RunId,
    lease_id: LeaseId,
    /// The run's own links: instance, request, correlation, task, run and lease.
    run_links: EventLinksDraft,
    /// The request alone, as the command that submitted the run links it.
    request_links: EventLinksDraft,
    /// The lease alone, as its release or expiry links it.
    lease_links: EventLinksDraft,
    events: Vec<ProjectedEvent>,
}

impl RunLedger {
    fn new() -> Self {
        let issuer = IdentifierIssuer::new().expect("identifier issuer");
        let request = issuer.mint_request_id().expect("request id");
        let run = issuer.mint_run_id().expect("run id");
        let lease = issuer.mint_lease_id().expect("lease id");
        let request_id = *request.transport();
        let run_id = *run.transport();
        let lease_id = *lease.transport();
        let request_links = EventLinksDraft::default().with_request_id(request.clone());
        let lease_links = EventLinksDraft::default().with_lease_id(lease.clone());
        let run_links = EventLinksDraft::default()
            .with_instance_id(issuer.mint_instance_id().expect("instance id"))
            .with_request_id(request)
            .with_correlation_id(issuer.mint_correlation_id().expect("correlation id"))
            .with_task_id(issuer.mint_task_id().expect("task id"))
            .with_run_id(run)
            .with_lease_id(lease);
        Self {
            issuer,
            request_id,
            run_id,
            lease_id,
            run_links,
            request_links,
            lease_links,
            events: Vec::new(),
        }
    }

    /// Records one event at the next sequence, as a Forensic read projects it.
    fn record(
        mut self,
        origin: EventOrigin,
        links: EventLinksDraft,
        payload: EventPayloadDraft,
    ) -> Self {
        let sequence = u64::try_from(self.events.len()).expect("ledger length") + 1;
        let sanitized = EventDraft::new(
            self.issuer.mint_event_id().expect("event id"),
            RECORDED_FROM_UNIX_MS + sequence,
            EventSeverity::Info,
            origin,
            links,
            payload,
        )
        .sanitize(&RejectProjectionSecrets)
        .expect("sanitize a scripted ledger event");
        self.events.push(ProjectedEvent {
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
        });
        self
    }

    fn runtime_origin() -> EventOrigin {
        EventOrigin::new(
            EventSource::Runtime,
            OriginModule::Runtime,
            EventActor::Runtime,
        )
    }

    fn scheduler_origin() -> EventOrigin {
        EventOrigin::new(
            EventSource::Scheduler,
            OriginModule::Scheduler,
            EventActor::Scheduler,
        )
    }

    /// The CLI's command that submitted the run: a manual run.
    fn cli_command(self) -> Self {
        let links = self.request_links.clone();
        self.record(
            EventOrigin::new(EventSource::Cli, OriginModule::Actingctl, EventActor::Cli),
            links,
            ClientPayloadDraft::cli_command(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        )
    }

    /// The scheduler's command that submitted the run: a scheduled run.
    fn scheduler_command(self) -> Self {
        let links = self.request_links.clone();
        self.record(
            Self::scheduler_origin(),
            links,
            CommandPayloadDraft::received(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        )
    }

    fn fact(self, fact: TaskSemanticFact) -> Self {
        let links = self.run_links.clone();
        self.record(
            Self::runtime_origin(),
            links,
            TaskPayloadDraft::semantic(fact, AuditInput::new()).into(),
        )
    }

    /// The run's package admission (`task.requested` with `PackageAdmitted`).
    fn admitted(self) -> Self {
        self.fact(TaskSemanticFact::PackageAdmitted {
            package_label: PACKAGE.to_owned(),
            task_label: TASK_LABEL.to_owned(),
            response_deadline_monotonic_ms: None,
        })
    }

    /// A `task.requested` whose payload is a plain request, not a package admission.
    fn requested(self) -> Self {
        let links = self.run_links.clone();
        self.record(
            Self::runtime_origin(),
            links,
            TaskPayloadDraft::requested(EventAction::RuntimeTaskRun, AuditInput::new()).into(),
        )
    }

    fn step_started(self) -> Self {
        self.fact(TaskSemanticFact::StepStarted {
            step_index: 0,
            operation_label: STEP.to_owned(),
            from_page: PAGE.to_owned(),
            phase: None,
        })
    }

    fn terminal(self, task_result: TaskOutcome, outcome: Option<Outcome>) -> Self {
        self.fact(TaskSemanticFact::TerminalCommitted {
            task_result,
            final_page: Some(PAGE.to_owned()),
            executed_steps: Some(1),
            outcome,
            failure_severe: false,
            scheduling_disposition: None,
            task_timing: None,
        })
    }

    fn runtime_started(self) -> Self {
        self.record(
            Self::runtime_origin(),
            EventLinksDraft::default(),
            RuntimePayloadDraft::started(EventAction::RuntimeStart, AuditInput::new()).into(),
        )
    }

    fn runtime_takeover(self) -> Self {
        self.record(
            Self::runtime_origin(),
            EventLinksDraft::default(),
            RuntimePayloadDraft::takeover(EventAction::RuntimeTakeover, AuditInput::new()).into(),
        )
    }

    fn lease_released(self) -> Self {
        let links = self.lease_links.clone();
        self.record(
            Self::scheduler_origin(),
            links,
            LeasePayloadDraft::released(
                EventAction::LeaseRelease,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        )
    }

    fn lease_expired(self) -> Self {
        let links = self.lease_links.clone();
        self.record(
            Self::scheduler_origin(),
            links,
            LeasePayloadDraft::expired(
                EventAction::LeaseExpire,
                EffectDisposition::Performed,
                AuditInput::new(),
            )
            .into(),
        )
    }

    /// The status of the run as a client reads it from this ledger by `key`.
    fn status(self, key: RunKey, mode: RunStatusMode) -> ContainedRunStatus {
        let events = self.events;
        let root = TempDir::new().expect("tempdir");
        let runtime = ScriptedRuntime::start(root.path(), move |_| {
            vec![ScriptedConnection::new().ledger(events)]
        });
        let client = connect(root.path(), IO);
        let status = client.contained_run_status(key, mode);
        drop(client);
        runtime.finish();
        status.unwrap_or_else(|error| panic!("the run status: {error:?}"))
    }

    fn by_request(self, mode: RunStatusMode) -> ContainedRunStatus {
        let key = RunKey::RequestId(self.request_id);
        self.status(key, mode)
    }
}

// G9 (R7): Workflow #338 R1. Only a package admission makes a run admitted; a step started
// after it makes it running; a terminal decides succeeded, failed or cancelled by its outcome,
// with its failure code and the cancellation reason `from_failure_code` gives. Brief mode
// carries no progress.
#[test]
fn gate_a_run_is_admitted_then_running_then_the_outcome_of_its_terminal() {
    let ledger = RunLedger::new().cli_command().requested();
    let status = ledger.by_request(RunStatusMode::Full);
    assert_ne!(
        status.state,
        ContainedRunState::Admitted,
        "a plain task request is no package admission"
    );

    let ledger = RunLedger::new().cli_command().admitted();
    let (request_id, run_id) = (ledger.request_id, ledger.run_id);
    let status = ledger.by_request(RunStatusMode::Full);
    assert_eq!(status.state, ContainedRunState::Admitted);
    assert_eq!(status.request_id, Some(request_id));
    assert_eq!(status.run_id, Some(run_id));
    assert_eq!(status.package_label.as_deref(), Some(PACKAGE));
    assert_eq!(status.evidence.admitted_sequence, Some(2));
    assert!(status.terminal.is_none());

    let running = || RunLedger::new().cli_command().admitted().step_started();
    let status = running().by_request(RunStatusMode::Full);
    assert_eq!(status.state, ContainedRunState::Running);
    assert_eq!(
        status.progress,
        Some(ContainedRunProgress {
            step_index: 0,
            operation_label: STEP.to_owned(),
            page: PAGE.to_owned(),
        })
    );
    let status = running().by_request(RunStatusMode::Brief);
    assert_eq!(status.state, ContainedRunState::Running);
    assert!(
        status.progress.is_none(),
        "brief mode: {:?}",
        status.progress
    );

    let failure = SharedCode::ContainedTaskHomeEntryNotMatched.code();
    let cancelled = RuntimeErrorCode::ContainedTaskCancelled.code();
    for (row, task_result, outcome, state, failure_code, reason) in [
        (
            "succeeded",
            TaskOutcome::Success,
            None,
            ContainedRunState::Succeeded,
            None,
            None,
        ),
        (
            "failed",
            TaskOutcome::Failure,
            Some(failure),
            ContainedRunState::Failed,
            Some(failure),
            None,
        ),
        (
            "cancelled",
            TaskOutcome::Cancelled,
            Some(cancelled),
            ContainedRunState::Cancelled,
            Some(cancelled),
            Some(ContainedTaskCancellationReason::ClientRequested),
        ),
    ] {
        let status = running()
            .terminal(task_result, outcome.map(Outcome::new))
            .by_request(RunStatusMode::Full);
        assert_eq!(status.state, state, "{row}");
        let terminal = status
            .terminal
            .unwrap_or_else(|| panic!("{row}: the run's terminal"));
        assert_eq!(terminal.outcome, task_result, "{row}");
        assert_eq!(
            terminal.failure_code.as_ref().map(|code| code.as_str()),
            failure_code.map(|code| code.as_str()),
            "{row}"
        );
        assert_eq!(terminal.cancellation_reason, reason, "{row}");
    }
}

// G9 (R7): Workflow #338 R1. A run without a terminal whose admission a later Runtime start
// or takeover follows is `interrupted_unterminated` (the two targeted queries the model names
// find it: neither event links a request); a terminal recorded after the restart decides the
// state instead.
#[test]
fn gate_a_run_without_a_terminal_after_a_runtime_start_or_takeover_is_interrupted_unterminated() {
    for (row, ledger) in [
        (
            "a later start",
            RunLedger::new().cli_command().admitted().runtime_started(),
        ),
        (
            "a later takeover",
            RunLedger::new().cli_command().admitted().runtime_takeover(),
        ),
    ] {
        let status = ledger.by_request(RunStatusMode::Full);
        assert_eq!(
            status.state,
            ContainedRunState::InterruptedUnterminated,
            "{row}"
        );
        assert!(status.evidence.restarted_after_admission, "{row}");
        assert!(status.terminal.is_none(), "{row}");
    }

    let status = RunLedger::new()
        .cli_command()
        .admitted()
        .runtime_started()
        .terminal(
            TaskOutcome::Cancelled,
            Some(Outcome::new(
                RuntimeErrorCode::ContainedTaskCancelled.code(),
            )),
        )
        .by_request(RunStatusMode::Full);
    assert_eq!(status.state, ContainedRunState::Cancelled);
}

// G9 (R7): a run the ledger does not hold is `not_found`. Its status always has the
// `request_id` member: the request asked for, or null when the run was asked for by run.
#[test]
fn gate_a_run_the_ledger_does_not_hold_is_not_found_with_its_request_member_always_present() {
    let ledger = RunLedger::new().runtime_started();
    let request_id = ledger.request_id;
    let status = ledger.by_request(RunStatusMode::Full);
    assert_eq!(status.state, ContainedRunState::NotFound);
    assert_eq!(status.request_id, Some(request_id));
    let value = serde_json::to_value(&status).expect("run status JSON");
    assert_eq!(
        value.get("request_id"),
        Some(&serde_json::to_value(request_id).expect("request id JSON"))
    );

    let ledger = RunLedger::new().runtime_started();
    let run_id = ledger.run_id;
    let status = ledger.status(RunKey::RunId(run_id), RunStatusMode::Full);
    assert_eq!(status.state, ContainedRunState::NotFound);
    assert_eq!(status.run_id, Some(run_id));
    assert_eq!(status.request_id, None);
    let value = serde_json::to_value(&status).expect("run status JSON");
    assert_eq!(value.get("request_id"), Some(&serde_json::Value::Null));
}

// G9 (R7): the request's own origin event names the dispatch and origin (a CLI command is
// manual, the scheduler's received command is scheduled), and the run's lease reads its
// terminal (released or expired) although that record does not link the request.
#[test]
fn gate_a_run_names_its_dispatch_its_origin_and_its_lease_terminal() {
    for (row, ledger, dispatch, origin, lease_terminal) in [
        (
            "manual",
            RunLedger::new().cli_command().admitted().lease_released(),
            RunDispatch::Manual,
            RunOrigin::Cli,
            ContainedTaskLeaseTerminal::Released,
        ),
        (
            "scheduled",
            RunLedger::new()
                .scheduler_command()
                .admitted()
                .lease_expired(),
            RunDispatch::Scheduled,
            RunOrigin::Scheduler,
            ContainedTaskLeaseTerminal::Expired,
        ),
    ] {
        let lease_id = ledger.lease_id;
        let status = ledger.by_request(RunStatusMode::Brief);
        assert_eq!(status.dispatch, dispatch, "{row}");
        assert_eq!(status.origin, origin, "{row}");
        assert_eq!(
            status.lease,
            Some(ContainedRunLease {
                lease_id,
                terminal: Some(lease_terminal),
            }),
            "{row}"
        );
    }
}
