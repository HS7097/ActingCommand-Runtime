// SPDX-License-Identifier: AGPL-3.0-only

//! W2-E (board #419; frozen model `rd-1024/iso-design/model.md` §5.2 R1, rows E13, A05, E02,
//! §5.3, §6; the W2-C review's M1): a run's input that meets a refusal of its instance's session
//! state fails only that input, as the same refusal at a capture or at a frame resolve does, so
//! it no longer pauses its scheduled task:
//!
//! - a: each of the three refusals, raised by X's real kernel, through the input's own mapping
//!   (`RuntimeHostError::input_backend`, with which `host/input.rs` builds the failure that
//!   `ActionFailure::backend` records; its `failure_severe` is that failure's `is_fatal`): the
//!   kernel's code, non-fatal, of X alone and stopping nothing, so the run's boundary contains
//!   nothing and only the input fails. A close of X's session merged into it with its resources
//!   confirmed is still carried; merged with an unconfirmed close it stays fatal, and the #403
//!   fence of X then takes it. A device fault (E02) and a session death at an input keep the
//!   kernel's severity;
//! - b: each refusal settled as a scheduled run's failure on the policy's own state, with the
//!   class the live settlement derives from it (`scheduled_failure_class`): `Recoverable`, a
//!   retry scheduled, the task not paused.
//!
//! Each refusal comes from the kernel's own check at an input of X, with no wait: the input runs
//! while another owner's close of X's session is in progress (inside that close, on the fake
//! input backend's close hook); the provider resolves X to another endpoint under its open
//! session; X's session holds no committed frame (the Nemu input's own check raises the same
//! code, and a fake opens no Nemu session). Nothing here sleeps or reads a clock.

use super::*;
use crate::FailureScope;
use crate::codes::HostLocation;
use crate::error::spelled;
use crate::host::failure_scope::SESSION_DEATH_CODES;
use crate::policy_control::{PolicyControlState, PolicyExecutionTiming};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::{
    FencedWrite, FencedWritePurpose, InputFrameReference, PerformanceContext, ResourceQuiescence,
    issue_fenced_write, scheduled_failure_class,
};
use actingcommand_device::{DeviceCloseAuthority, DeviceResourceQuiescence};
use actingcommand_execution_kernel::{ExecutionKernelCode, ExecutionKernelError};
use actingcommand_policy::{CompiledCatalog, DispatchPrerequisites};

const X_ALIAS: &str = "w2e.instance.x";

/// A business step of a lease on X, as the host admits an input with.
#[cfg(test)]
fn business_step(x: InstanceId) -> Arc<FencedWrite> {
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    Arc::new(issue_fenced_write(
        LeaseToken::new(
            *ids.mint_owner_epoch().expect("owner epoch").transport(),
            *ids.mint_lease_id().expect("lease").transport(),
            x,
            *ids.mint_holder_id().expect("holder").transport(),
            100,
        )
        .expect("a lease token of X"),
        1,
        std::num::NonZeroU64::new(1).expect("step"),
        FencedWritePurpose::Business,
    ))
}

/// X's reset, as the host's input runs it on the kernel (`input_prepared_in_frame`).
fn input_on_x(kernel: &ExecutionKernel, x: InstanceId) -> Result<(), ExecutionKernelError> {
    kernel
        .input_prepared_in_frame(
            X_ALIAS,
            kernel
                .prepare_input(InputAction::Reset)
                .expect("a reset prepares"),
            None,
            None,
            business_step(x),
            (),
        )
        .map(|_| ())
}

/// X's kernel on `provider`, with X's session open: one input of X ran.
fn open_kernel(provider: FakeProvider, x: InstanceId) -> Arc<ExecutionKernel> {
    let kernel = Arc::new(ExecutionKernel::new(Arc::new(provider)));
    input_on_x(&kernel, x).expect("X's first input opens its session");
    kernel
}

/// The three refusals of X's session state, each raised at X's input by a kernel of its own:
/// a close in progress, a changed endpoint and a frame not committed, in that order.
fn refusals(x: InstanceId) -> [ExecutionKernelError; 3] {
    // Another owner closes X's session; X's input runs while that close is in progress.
    let state = Arc::new(FakeState::default());
    let kernel = open_kernel(FakeProvider::one(X_ALIAS, x, Arc::clone(&state)), x);
    let (sender, receiver) = mpsc::channel();
    let during_close = Arc::clone(&kernel);
    let hook: Box<dyn FnOnce() + Send> = Box::new(move || {
        sender
            .send(input_on_x(&during_close, x))
            .expect("the test takes the input's result");
    });
    *state.input_close_hook.lock().expect("input close hook") = Some(hook);
    kernel
        .close_instance(x, DeviceCloseAuthority::LocalOnly)
        .expect("the other owner's close of X's session completes");
    let closing = receiver
        .try_recv()
        .expect("X's input ran while the close was in progress")
        .expect_err("a closing session admits no input");
    assert_eq!(
        closing.code().as_str(),
        ExecutionKernelCode::ExecutionSessionCloseInProgress.as_str()
    );

    // X's endpoint changes under its open session.
    let resolved = Arc::new(std::sync::Mutex::new(ResolvedExecutionInstance::new(
        x,
        "127.0.0.1:16384",
    )));
    let kernel = open_kernel(
        FakeProvider::one(X_ALIAS, x, Arc::new(FakeState::default()))
            .with_resolved_override(Arc::clone(&resolved)),
        x,
    );
    *resolved.lock().expect("X's resolution") =
        ResolvedExecutionInstance::new(x, "127.0.0.1:16416");
    let changed = input_on_x(&kernel, x).expect_err("X's endpoint changed under its session");
    assert_eq!(
        changed.code().as_str(),
        ExecutionKernelCode::ExecutionInstanceIdentityMismatch.as_str()
    );

    // X's input frame is not the one its session committed.
    let ids = IdentifierIssuer::new().expect("identifier issuer");
    let kernel = open_kernel(
        FakeProvider::one(X_ALIAS, x, Arc::new(FakeState::default())),
        x,
    );
    let uncommitted = kernel
        .resolve_input_frame(
            X_ALIAS,
            InputFrameReference {
                frame_id: *ids.mint_frame_id().expect("frame id").transport(),
                width: 16,
                height: 9,
            },
        )
        .expect_err("X's session committed no frame");
    assert_eq!(
        uncommitted.code().as_str(),
        ExecutionKernelCode::InputFrameNotCommitted.as_str()
    );

    [closing, changed, uncommitted]
}

/// The failed close of X's session whose input backend's resources are `quiescence`, from X's
/// real kernel: the close that a failed input's cleanup runs (`finish_input_failure`) and merges
/// into the input's failure.
fn close_failure(x: InstanceId, quiescence: DeviceResourceQuiescence) -> ExecutionKernelError {
    let unconfirmed = quiescence == DeviceResourceQuiescence::Unconfirmed;
    let state = Arc::new(FakeState::default());
    let kernel = open_kernel(FakeProvider::one(X_ALIAS, x, Arc::clone(&state)), x);
    *state.close_error.lock().expect("close error") = Some(
        DeviceError::fatal("injected close failure of X's input backend")
            .with_resource_summary(quiescence, 1),
    );
    let failure = kernel
        .close_instance(x, DeviceCloseAuthority::LocalOnly)
        .expect_err("X's input backend fails to close");
    // The kernel is closed before it is dropped, as the host's shutdown closes its kernel: an
    // unconfirmed close stays retained and that close reports it, a confirmed one is settled. A
    // kernel dropped unclosed runs its close in `Drop` and panics on such a failure.
    let closed = kernel.close();
    if unconfirmed {
        closed.expect_err("the kernel's close reports X's retained unconfirmed close");
    } else {
        closed.expect("X's confirmed close is settled");
    }
    failure
}

/// a (R1; rows E13, A05, E02): the three refusals at X's input fail only the input, a merged
/// confirmed close is still carried, an unconfirmed one keeps the input severe, and a device
/// fault or a session death at an input keeps the kernel's severity.
#[test]
fn w2e_a_a_closing_or_changed_session_or_an_uncommitted_frame_fails_only_the_input() {
    let x = instance_id();
    let refusals = refusals(x);
    let confirmed = close_failure(x, DeviceResourceQuiescence::Confirmed);
    let unconfirmed = close_failure(x, DeviceResourceQuiescence::Unconfirmed);
    assert_eq!(
        confirmed.resource_quiescence(),
        Some(ResourceQuiescence::Confirmed)
    );
    assert_eq!(
        unconfirmed.resource_quiescence(),
        Some(ResourceQuiescence::Unconfirmed)
    );

    for refusal in &refusals {
        let code = refusal.code().as_str();
        assert!(
            RuntimeHostError::execution(HostLocation::ExecuteInputBackend, refusal).is_fatal(),
            "{code}: the kernel's own severity, which paused the task before"
        );
        let failure =
            RuntimeHostError::input_backend(HostLocation::ExecuteInputBackend, refusal, x);
        assert_eq!(failure.code(), code);
        assert!(!failure.is_fatal(), "{code}: the input's failure_severe");
        assert!(!failure.halts_runtime(), "{code}");
        assert_eq!(
            failure.scope_at(Some(x)),
            FailureScope::Instance(x),
            "{code}"
        );

        // The input's cleanup closed X's session with its resources confirmed: the input still
        // fails alone, and the close's failure stays on it, so it is recorded (W1-7 M1).
        let merged = ExecutionKernelError::merge_cleanup(refusal.clone(), confirmed.clone());
        let failure =
            RuntimeHostError::input_backend(HostLocation::ExecuteInputBackend, &merged, x);
        assert_eq!(failure.code(), code);
        assert!(!failure.is_fatal(), "{code} with a confirmed close");
        assert_eq!(
            failure.cleanup_cause().map(|cause| cause.code().as_str()),
            Some(confirmed.code().as_str()),
            "{code}: the merged close failure is carried"
        );

        // An unconfirmed close keeps the input severe: the input's settlement fences X (#403).
        let merged = ExecutionKernelError::merge_cleanup(refusal.clone(), unconfirmed.clone());
        assert_eq!(
            merged.resource_quiescence(),
            Some(ResourceQuiescence::Unconfirmed)
        );
        let failure =
            RuntimeHostError::input_backend(HostLocation::ExecuteInputBackend, &merged, x);
        assert_eq!(failure.code(), code);
        assert!(failure.is_fatal(), "{code} with an unconfirmed close");
    }

    // E02: X's device fails the input.
    let state = Arc::new(FakeState::default());
    let kernel = open_kernel(FakeProvider::one(X_ALIAS, x, Arc::clone(&state)), x);
    state.fail_input.store(true, Ordering::Release);
    let fault = input_on_x(&kernel, x).expect_err("X's device fails the input");
    assert_eq!(
        fault.code().as_str(),
        SharedCode::InputBackendOperationFailed.as_str()
    );
    assert!(
        RuntimeHostError::input_backend(HostLocation::ExecuteInputBackend, &fault, x).is_fatal(),
        "a device fault at an input keeps its severity (E02)"
    );

    // A session death at an input: X's session thread died in a retained capture.
    let state = Arc::new(FakeState::default());
    let kernel = open_kernel(FakeProvider::one(X_ALIAS, x, Arc::clone(&state)), x);
    state.panic_capture_once.store(true, Ordering::Release);
    assert!(
        kernel.capture_retained(X_ALIAS).is_err(),
        "X's session thread panics in the capture"
    );
    let death = input_on_x(&kernel, x).expect_err("X's session is dead");
    assert!(
        spelled(death.code().as_str(), SESSION_DEATH_CODES),
        "{}",
        death.code().as_str()
    );
    assert!(
        RuntimeHostError::input_backend(HostLocation::ExecuteInputBackend, &death, x).is_fatal(),
        "a session death at an input keeps its severity until an input's deaths are counted"
    );
    // Closed before it is dropped, as the host's shutdown closes its kernel: X's dead session
    // is still registered, and its close reports the death.
    kernel
        .close()
        .expect_err("the kernel's close reports X's dead session");
}

/// A dispatch of the fixture catalog's first task on its instance, admitted at the fixture's
/// time.
fn dispatch_intent(catalog: &CompiledCatalog, suffix: u64) -> DispatchIntent {
    let task = &catalog.catalog().tasks.tasks[0];
    DispatchIntent {
        decision_id: format!("decision:w2e-{suffix}"),
        task_id: task.id.clone(),
        instance_id: POLICY_INSTANCE_ALIAS.to_owned(),
        operation_id: task.entrypoint.operation_id.clone(),
        procedure_ref: task.procedure_ref.clone(),
        procedure_binding_digest: Some(format!("sha256:{}", "b".repeat(64))),
        catalog_hash: catalog.catalog_hash().to_owned(),
        catalog_version: catalog.catalog().tasks.catalog.catalog_version,
        input_ledger_position: 1,
        fact_snapshot_id: "snapshot:w2e".to_owned(),
        approval_refs: catalog.catalog().tasks.catalog.approval_refs.clone(),
        reason_chain_id: format!("reason:w2e-{suffix}"),
        expected_duration_ms: task.expected_duration_ms,
        yield_points: task.yield_points.clone(),
        load_profile: LoadProfile::Light,
        prerequisites: DispatchPrerequisites {
            fencing_required: true,
            evaluated_at_unix_ms: POLICY_NOW_UNIX_MS,
            facts_fresh_until_unix_ms: None,
            activity_profile_id: "fixture-activity-a".to_owned(),
            daily_limit: task.daily_limit(),
            window_iteration_limit: task.window_iteration_limit(),
            max_runtime_ms: task.max_runtime_ms(),
            urgency_milli: 0,
        },
    }
}

/// b (R1; rows E13, A05; W2-C's settlement): a scheduled run whose input met one of the three
/// refusals fails once; its terminal carries the input's failure (the kernel's code, and the
/// failure's `is_fatal` as `failure_severe`), and the policy settles it with the class the live
/// settlement derives (`scheduled_failure_class`): `Recoverable`, a retry scheduled, so the task
/// is held only by its back-off and is admitted again at the retry time. (With an unconfirmed
/// close merged into it the input stays fatal, `a`, and the #403 fence of X takes it before the
/// run settles; that path is #403's and is not settled here.)
#[test]
fn w2e_b_a_scheduled_runs_input_refused_by_its_session_does_not_pause_its_task() {
    let x = instance_id();
    let catalog = compile_catalog(&policy_sources(1)).expect("the fixture catalog");
    for refusal in refusals(x) {
        let code = refusal.code().as_str();
        let failure =
            RuntimeHostError::input_backend(HostLocation::ExecuteInputBackend, &refusal, x);
        let outcome = failure.outcome();
        let class = scheduled_failure_class(outcome.code(), failure.is_fatal());
        let mut state = PolicyControlState::default();
        let first = dispatch_intent(&catalog, 1);
        let admission = state
            .preview_admission(&catalog, &first, POLICY_NOW_UNIX_MS)
            .expect("admission preview");
        let data = state
            .preview_execution(
                &catalog,
                &first,
                &admission,
                PolicyExecutionTiming {
                    observed_at_unix_ms: POLICY_NOW_UNIX_MS + 100,
                    runtime_ms: 100,
                },
                &PolicyExecutionInput::Failed {
                    outcome,
                    failure_identity: None,
                    class,
                },
                &PerformanceContext::unavailable(POLICY_NOW_UNIX_MS + 100),
            )
            .expect("failure classification");
        let PolicyExecutionOutcome::Failed { failure: record } = &data.outcome else {
            panic!("{code}: expected a failure")
        };
        assert_eq!(record.outcome.code().as_str(), code);
        state
            .commit_execution(&catalog, &first, &admission, &data)
            .expect("commit the failure");
        let next = dispatch_intent(&catalog, 2);
        let held = state
            .preview_admission(&catalog, &next, data.observed_at_unix_ms)
            .expect_err("the failure holds the pair")
            .code();
        assert_eq!(
            record.original_class,
            PolicyFailureClass::Recoverable,
            "{code}"
        );
        assert_eq!(
            record.effective_class,
            PolicyFailureClass::Recoverable,
            "{code}"
        );
        assert_eq!(
            record.disposition,
            PolicyFailureDisposition::RetryScheduled,
            "{code}"
        );
        assert_eq!(
            held, "policy_retry_backoff_active",
            "{code}: the task is not paused"
        );
        let retry_at = record.retry_at_unix_ms.expect("a retry time");
        assert!(
            state.preview_admission(&catalog, &next, retry_at).is_ok(),
            "{code}: the pair is admitted again at its retry time"
        );
    }
}
