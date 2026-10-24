// SPDX-License-Identifier: AGPL-3.0-only

//! ISO-1 (board #419, frozen model `rd-1024/iso-design/model.md` §5-§6; Alice 10-24: one
//! emulator's fault must not stop the whole Runtime): the failure scope core.
//!
//! - The classification table (R0.1-R0.4): every code of the closed Global list is `Global`,
//!   however its constructor tagged it, and so is a catalog code of category `fatal`; a device,
//!   lease or run failure tagged as one instance's is `Instance` of that instance, else of its
//!   boundary's, and `Global` without either; an untagged fatal failure stays `Global` (the
//!   fail-safe default); a request failure is `Operation`; a join with a `Global` failure is
//!   `Global`; a poisoned lock of one instance alone is cleared and fails as that instance's.
//! - `contain_failure` (R0.2, R0.5-R0.7), each case on its own host with instances X and Y and a
//!   manual clock: an `Instance` failure of X fences X only (its fence record and status, no
//!   latch, no shutdown) and Y's direct run completes after it; an `Operation` failure fails only
//!   its operation; a `Global` failure latches the Runtime; a fence that fails latches.
//! - The receipt (#411 A54, §5.4): a failed receipt says `host_halted` only when the Runtime
//!   latched. A read-only observe whose capture fails with a fatal device error, and an IPC input
//!   whose backend fails, latch nothing, so their receipts say `host_halted` false.
//!
//! X's stuck recovery is off, so its fence waits for a person and no ladder acts on X while the
//! test reads it. Nothing here sleeps or waits for a background step.

use super::gate_403::{assert_fence, fence_records, status_fence};
use super::instance_workers::all_events;
use super::*;
use crate::FailureScope;
use crate::codes::test_spellings::TestLocation;
use crate::codes::{HostCode, HostLocation};
use crate::error::RuntimeFailureRelation;
use crate::host::failure_scope::{
    Contained, GLOBAL_CODES, GlobalFamily, global_family, lock_instance,
};
use actingcommand_contract::InstanceStuckRecovery;
use actingcommand_contract::codes::{ContractCode, SharedCode};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;

const X_ALIAS: &str = "iso1.instance.x";
const Y_ALIAS: &str = "iso1.instance.y";
const RECEIPT_ALIAS: &str = "iso1.instance.receipt";
const LOCATION: TestLocation = TestLocation::ContainFailureForTest;

/// The families of the closed Global list that name their codes (R0.3); `CatalogFatal` takes
/// its codes from the catalog.
const LISTED_FAMILIES: [GlobalFamily; 10] = [
    GlobalFamily::Ledger,
    GlobalFamily::Configuration,
    GlobalFamily::Owner,
    GlobalFamily::Identifier,
    GlobalFamily::Clock,
    GlobalFamily::SharedLock,
    GlobalFamily::ProcessResource,
    GlobalFamily::SchedulerInvariant,
    GlobalFamily::PolicyProjection,
    GlobalFamily::Installer,
];

/// A started host with X and Y on a manual clock. X's stuck recovery is off; Y has a direct-run
/// package.
struct TwoInstances {
    host: RuntimeHost,
    x: InstanceId,
    y_package: String,
    _root: TempDir,
}

fn two_instances() -> TwoInstances {
    let root = TempDir::new().expect("tempdir");
    let x_state = Arc::new(FakeState::default());
    let y_state = Arc::new(FakeState::default());
    let y_package = super::gate_369s5::direct_run_package(&root, &y_state);
    let x = instance_id();
    let host = RuntimeHost::start(
        config(&root)
            .with_runtime_clock(Arc::new(ManualRuntimeClock::new(1_000, 0)))
            .with_stuck_recovery(BTreeMap::from([(
                X_ALIAS.to_owned(),
                InstanceStuckRecovery {
                    enabled: false,
                    cooldown_secs: 600,
                },
            )])),
        Arc::new(FakeProvider::from_entries([
            (X_ALIAS.to_owned(), x, x_state),
            (Y_ALIAS.to_owned(), instance_id(), y_state),
        ])),
    )
    .expect("runtime host");
    TwoInstances {
        host,
        x,
        y_package,
        _root: root,
    }
}

impl TwoInstances {
    /// The Runtime holds no fatal state and no shutdown was requested.
    fn assert_running(&self) {
        assert!(
            self.host.fatal_error().expect("runtime health").is_none(),
            "the Runtime latched"
        );
        assert!(
            !self.host.is_shutdown_requested().expect("shutdown state"),
            "a shutdown was requested"
        );
    }

    /// The Runtime latched `code`, and a shutdown was requested.
    fn assert_latched(&self, code: &str) {
        let latched = self
            .host
            .fatal_error()
            .expect("runtime health")
            .expect("the Runtime latched");
        assert_eq!(latched.code(), code, "{latched:?}");
        assert!(self.host.is_shutdown_requested().expect("shutdown state"));
    }

    /// Y's direct run is dispatched and completes.
    fn assert_y_run_completes(&self) {
        let ids = IdentifierIssuer::new().expect("identifier issuer");
        let run = runtime_request(
            &ids,
            RuntimeOperation::run_contained_task(
                Y_ALIAS,
                ids.mint_holder_id().expect("holder"),
                ContainedTaskRequest::new(self.y_package.clone()).expect("task request"),
            ),
        );
        let receipt = self
            .host
            .process_request_for_test(&run, ConnectionId::new(4191).expect("connection"))
            .expect("Y's run receipt");
        assert_eq!(
            receipt.state(),
            RuntimeReceiptState::Completed,
            "{receipt:?}"
        );
        assert!(
            matches!(
                receipt.result(),
                Some(RuntimeResult::ContainedTaskCompleted {
                    task_result: TaskOutcome::Success,
                    ..
                })
            ),
            "{receipt:?}"
        );
    }
}

/// A failure that originates in `instance` (R0.6: its capture backend failed, its state is
/// unknown).
fn capture_fault(instance: InstanceId) -> RuntimeHostError {
    RuntimeHostError::instance_fault(
        SharedCode::CaptureBackendOperationFailed,
        LOCATION,
        RuntimeErrorCode::CaptureFailed,
        instance,
    )
}

/// The ledger failure every Global test latches with.
fn ledger_failure() -> RuntimeHostError {
    RuntimeHostError::fatal(
        RuntimeErrorCode::LedgerFailure,
        LOCATION,
        RuntimeErrorCode::LedgerFailure,
    )
}

/// The classification table (R0.1-R0.4, R8).
#[test]
fn iso1_a_the_scope_classification_table() {
    let x = instance_id();
    let y = instance_id();

    // Every code of the closed Global list is Global: built fatal and untagged, at a boundary
    // that knows an instance, and tagged as one instance's.
    let mut families = Vec::new();
    for (index, entry) in GLOBAL_CODES.iter().enumerate() {
        let spelling = entry.code.as_str();
        assert!(
            GLOBAL_CODES[..index]
                .iter()
                .all(|earlier| earlier.code.as_str() != spelling),
            "{spelling} is listed twice"
        );
        assert_eq!(global_family(spelling), Some(entry.family), "{spelling}");
        let untagged =
            RuntimeHostError::fatal(entry.code, LOCATION, RuntimeErrorCode::RuntimeHalted);
        assert_eq!(
            untagged.scope_at(Some(x)),
            FailureScope::Global,
            "{spelling}"
        );
        let tagged = RuntimeHostError::instance_fault(
            entry.code,
            LOCATION,
            RuntimeErrorCode::RuntimeHalted,
            x,
        );
        assert_eq!(tagged.scope(), FailureScope::Global, "{spelling}");
        assert_eq!(tagged.scope_at(Some(y)), FailureScope::Global, "{spelling}");
        assert!(tagged.halts_runtime(), "{spelling}");
        if !families.contains(&entry.family) {
            families.push(entry.family);
        }
    }
    for family in LISTED_FAMILIES {
        assert!(families.contains(&family), "{family:?} lists no code");
    }

    // A catalog code of category `fatal` is Global, tagged as one instance's too.
    let panic_unrecovered = ContractCode::PanicUnrecovered;
    assert_eq!(
        global_family(panic_unrecovered.as_str()),
        Some(GlobalFamily::CatalogFatal)
    );
    let tagged = RuntimeHostError::instance_fault(
        panic_unrecovered,
        LOCATION,
        RuntimeErrorCode::RuntimeHalted,
        x,
    );
    assert_eq!(tagged.scope_at(Some(x)), FailureScope::Global);

    // A device, lease or run failure tagged as one instance's is Instance of that instance,
    // whatever the boundary's instance; it is severe for its own work and stops no Runtime.
    for code in [
        SharedCode::CaptureBackendOperationFailed.code(),
        RuntimeErrorCode::LeaseExpired.code(),
        SharedCode::ContainedTaskStateInvalid.code(),
    ] {
        let fault =
            RuntimeHostError::instance_fault(code, LOCATION, RuntimeErrorCode::RuntimeHalted, x);
        assert_eq!(fault.scope(), FailureScope::Instance(x), "{code}");
        assert_eq!(fault.scope_at(Some(y)), FailureScope::Instance(x), "{code}");
        assert!(fault.is_fatal(), "{code}");
        assert!(!fault.halts_runtime(), "{code}");
        // The wrappers keep the scope.
        assert_eq!(
            fault.into_fatal().scope_at(None),
            FailureScope::Instance(x),
            "{code}"
        );
    }

    // A failure tagged as one instance's that names none takes the boundary's instance, and is
    // Global without one.
    let unnamed = RuntimeHostError::fatal(
        SharedCode::CaptureBackendOperationFailed,
        LOCATION,
        RuntimeErrorCode::CaptureFailed,
    )
    .instance_scoped();
    assert_eq!(unnamed.scope_at(Some(y)), FailureScope::Instance(y));
    assert_eq!(unnamed.scope(), FailureScope::Global);

    // An unclassified fatal failure stays Global, even at a boundary that knows its instance
    // (R0.4, the fail-safe default).
    let unclassified = RuntimeHostError::fatal(
        SharedCode::CaptureBackendOperationFailed,
        LOCATION,
        RuntimeErrorCode::CaptureFailed,
    );
    assert_eq!(unclassified.scope_at(Some(x)), FailureScope::Global);
    assert!(unclassified.halts_runtime());

    // A request failure is Operation.
    let request = RuntimeHostError::request(
        RuntimeErrorCode::InvalidRequest,
        LOCATION,
        RuntimeErrorCode::InvalidRequest,
    );
    assert_eq!(request.scope_at(Some(x)), FailureScope::Operation);
    assert!(!request.halts_runtime());

    // A join (B7) with a Global failure is Global; one with an Operation failure keeps the
    // instance.
    let joined_global = capture_fault(x)
        .with_complete_failure(RuntimeFailureRelation::LifecycleRecord, ledger_failure());
    assert_eq!(joined_global.scope_at(Some(x)), FailureScope::Global);
    let joined_operation =
        capture_fault(x).with_complete_failure(RuntimeFailureRelation::LifecycleRecord, request);
    assert_eq!(joined_operation.scope(), FailureScope::Instance(x));

    // R8: a poisoned lock of one instance alone is cleared, and the work that met it fails as
    // that instance's failure; the next lock is taken.
    let instance_lock = Mutex::new(());
    let poisoned = catch_unwind(AssertUnwindSafe(|| {
        let _guard = instance_lock.lock().expect("the instance's lock");
        panic!("a panic while the instance's lock is held");
    }));
    assert!(poisoned.is_err());
    assert!(instance_lock.is_poisoned());
    let error = lock_instance(&instance_lock, LOCATION, x).expect_err("a poisoned lock fails");
    assert_eq!(error.code(), HostCode::InstanceLockPoisoned.as_str());
    assert_eq!(error.scope(), FailureScope::Instance(x));
    assert!(!error.halts_runtime());
    assert!(!instance_lock.is_poisoned(), "the poison is cleared");
    drop(lock_instance(&instance_lock, LOCATION, x).expect("the cleared lock is taken"));
}

/// R0.2, R0.5: an Instance failure of X fences X only. Its fence is recorded once, caused by
/// the failure, and shown in the status; the Runtime latches nothing and requests no shutdown;
/// the work ends with the fence's non-fatal failure; Y's direct run completes after it.
#[test]
fn iso1_b_contain_failure_fences_an_instance_failure_and_the_other_instance_runs() {
    let fixture = two_instances();
    let cause = SharedCode::CaptureBackendOperationFailed.as_str();

    let contained = fixture
        .host
        .contain_failure_for_test(capture_fault(fixture.x), None)
        .expect("contain X's failure");

    let fence = match contained {
        Contained::Fenced(fence) => fence,
        other => panic!("X's failure fences X: {other:?}"),
    };
    assert_eq!(fence.code(), HostCode::InstanceResourcesFenced.as_str());
    assert!(!fence.is_fatal(), "{fence:?}");
    fixture.assert_running();
    let status = status_fence(&fixture.host, X_ALIAS).expect("X's fence in the status");
    assert_eq!(status.cause.as_str(), cause, "{status:?}");
    assert_eq!(status_fence(&fixture.host, Y_ALIAS), None);
    let events = all_events(&fixture.host);
    let fences = fence_records(&events);
    let [(record, outcome)] = fences.as_slice() else {
        panic!("one fence record: {fences:#?}");
    };
    assert_eq!(record.links().instance_id(), Some(&fixture.x));
    assert_fence(outcome, fixture.x, cause);
    fixture.assert_y_run_completes();
    fixture.assert_running();
    fixture.host.close().expect("close host");
}

/// R0.6: an Operation failure ends only its operation, even at a boundary bound to X: it comes
/// back as it was, nothing is fenced or latched, and Y's direct run completes.
#[test]
fn iso1_c_contain_failure_fails_only_the_operation_of_an_operation_failure() {
    let fixture = two_instances();
    let failure = RuntimeHostError::request(
        RuntimeErrorCode::InvalidRequest,
        LOCATION,
        RuntimeErrorCode::InvalidRequest,
    );

    let contained = fixture
        .host
        .contain_failure_for_test(failure.clone(), Some(fixture.x))
        .expect("contain the operation's failure");

    let failed = match contained {
        Contained::Failed(failed) => failed,
        other => panic!("the operation fails alone: {other:?}"),
    };
    assert_eq!(failed, failure);
    fixture.assert_running();
    assert_eq!(status_fence(&fixture.host, X_ALIAS), None);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    fixture.assert_y_run_completes();
    fixture.host.close().expect("close host");
}

/// R0.2: a Global failure (a ledger failure) latches the Runtime, even at a boundary bound to
/// X, and fences nothing.
#[test]
fn iso1_d_contain_failure_latches_a_global_failure() {
    let fixture = two_instances();

    let contained = fixture
        .host
        .contain_failure_for_test(ledger_failure(), Some(fixture.x))
        .expect("contain the ledger failure");

    let latched = match contained {
        Contained::Latched(latched) => latched,
        other => panic!("a Global failure latches: {other:?}"),
    };
    let ledger = RuntimeErrorCode::LedgerFailure.as_str();
    assert_eq!(latched.code(), ledger);
    assert!(latched.is_fatal());
    fixture.assert_latched(ledger);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    let closed = fixture
        .host
        .close()
        .expect_err("the close ends with the latched condition");
    assert_eq!(closed.code(), ledger);
}

/// R0.7: when the fence of X's Instance failure fails (its ledger record), the fence's failure
/// latches the Runtime: containment never fails silently.
#[test]
fn iso1_e_a_fence_that_fails_latches_the_runtime() {
    let fixture = two_instances();
    fixture
        .host
        .fail_next_fence_for_test()
        .expect("arm the fence failure");

    let contained = fixture
        .host
        .contain_failure_for_test(capture_fault(fixture.x), None)
        .expect("contain X's failure");

    let latched = match contained {
        Contained::Latched(latched) => latched,
        other => panic!("a failed fence latches: {other:?}"),
    };
    let ledger = RuntimeErrorCode::LedgerFailure.as_str();
    assert_eq!(latched.code(), ledger);
    assert_eq!(
        latched.operation(),
        HostLocation::FenceInstanceResources.as_str()
    );
    fixture.assert_latched(ledger);
    assert!(fence_records(&all_events(&fixture.host)).is_empty());
    let closed = fixture
        .host
        .close()
        .expect_err("the close ends with the latched condition");
    assert_eq!(closed.code(), ledger);
}

/// #411 A54 (§5.4): a failed receipt says `host_halted` only when the Runtime latched. A
/// read-only observe whose capture fails with a fatal device error, and an IPC input whose
/// backend fails, latch nothing: both receipts are Failed with `host_halted` false, the observe's
/// with `capture_failed`, the input's with `backend_operation_failed`.
#[test]
fn iso1_f_a_receipt_says_host_halted_only_when_the_runtime_latched() {
    let root = TempDir::new().expect("tempdir");
    let state = Arc::new(FakeState::default());
    let host = host_with_state(&root, RECEIPT_ALIAS, Arc::clone(&state));
    let mut client = TestClient::connect(&host);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: RECEIPT_ALIAS.to_owned(),
    });
    let opened = client.send(&observe);
    assert_eq!(opened.state(), RuntimeReceiptState::Completed, "{opened:?}");

    state.fail_capture.store(true, Ordering::Release);
    let observe = client.request(RuntimeOperation::ObserveReadonly {
        instance_alias: RECEIPT_ALIAS.to_owned(),
    });
    let failed = client.send(&observe);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    let projection = failed.error_projection().expect("the observe's failure");
    assert_eq!(
        projection.code,
        RuntimeErrorCode::CaptureFailed,
        "{failed:?}"
    );
    assert!(!projection.fatal, "{failed:?}");
    assert_eq!(
        failed.outcome().value("host_halted"),
        Some(&serde_json::json!(false)),
        "{failed:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());

    state.fail_capture.store(false, Ordering::Release);
    state.fail_input.store(true, Ordering::Release);
    let (_, token) = client.acquire(RECEIPT_ALIAS);
    let input = client.request(RuntimeOperation::Input {
        frame: None,
        token,
        action: InputAction::Reset,
    });
    let failed = client.send(&input);

    assert_eq!(failed.state(), RuntimeReceiptState::Failed, "{failed:?}");
    let projection = failed.error_projection().expect("the input's failure");
    assert_eq!(
        projection.code,
        RuntimeErrorCode::BackendOperationFailed,
        "{failed:?}"
    );
    assert!(!projection.fatal, "{failed:?}");
    assert_eq!(
        failed.outcome().value("host_halted"),
        Some(&serde_json::json!(false)),
        "{failed:?}"
    );
    assert!(host.fatal_error().expect("runtime health").is_none());
    drop(client);
    host.close().expect("close host");
}
