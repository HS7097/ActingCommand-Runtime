// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 KB-1 fix round 1 (0.12.69; register §1.447; the coordinator's ruling R5 of 11-16,
//! the review's (h)): the kernel's half of the frame-bound key path. A session that gives its
//! input the step's context (the Nemu pair's) hands a key to that input as `key_in_frame`, with
//! the step's context; a session that gives none, as the plain `key`. The device registry's half is
//! the runtime host's `provider/gate_463_kb1.rs`: no fake provider opens a Nemu session, so no host
//! test drives a key through both. Nothing here opens a backend, sleeps or reads a clock.

use super::*;
use actingcommand_device::{DeviceResourceCloseOutcome, InputCheckPhase};

/// What a step's check answers is left of its deadline; the input reports it back, so the context
/// it was given is the step's.
const STEP_REMAINING_MS: u64 = 4_632;

/// A step's check that always answers [`STEP_REMAINING_MS`].
struct StepCheck;

impl InputOperationCheck for StepCheck {
    fn check(&self, _phase: InputCheckPhase) -> DeviceResult<Duration> {
        Ok(Duration::from_millis(STEP_REMAINING_MS))
    }
}

/// One call the input saw: its name, its key and, for a key on a frame, what the context's check
/// answered.
type KeyCall = (&'static str, String, Option<Duration>);

/// The session's input: it records each key and refuses every other input, which no test sends.
#[derive(Default)]
struct KeyRecorder {
    calls: Vec<KeyCall>,
}

fn not_sent() -> DeviceError {
    DeviceError::fatal("this test sends keys alone")
}

impl InputBackend for KeyRecorder {
    fn tap(&mut self, _witness: &FencedWrite, _x: i32, _y: i32) -> DeviceResult<()> {
        Err(not_sent())
    }

    fn long_tap(
        &mut self,
        _witness: &FencedWrite,
        _x: i32,
        _y: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Err(not_sent())
    }

    fn swipe(
        &mut self,
        _witness: &FencedWrite,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        Err(not_sent())
    }

    fn key(&mut self, _witness: &FencedWrite, key: &str) -> DeviceResult<()> {
        self.calls.push(("key", key.to_owned(), None));
        Ok(())
    }

    fn key_in_frame(
        &mut self,
        _witness: &FencedWrite,
        key: &str,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        let remaining = context.check.check(InputCheckPhase::Continue)?;
        self.calls
            .push(("key_in_frame", key.to_owned(), Some(remaining)));
        Ok(())
    }

    fn text(&mut self, _witness: &FencedWrite, _text: &str) -> DeviceResult<()> {
        Err(not_sent())
    }

    fn reset(&mut self, _witness: &FencedWrite) -> DeviceResult<()> {
        Err(not_sent())
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Err(not_sent())
    }
}

/// A scheduler-shaped step witness for driving the input directly.
#[cfg(test)]
fn step_witness() -> FencedWrite {
    let issuer = actingcommand_contract::IdentifierIssuer::new().expect("ids");
    actingcommand_contract::issue_fenced_write(
        actingcommand_contract::LeaseToken::new(
            *issuer.mint_owner_epoch().expect("epoch").transport(),
            *issuer.mint_lease_id().expect("lease").transport(),
            *issuer.mint_instance_id().expect("instance").transport(),
            *issuer.mint_holder_id().expect("holder").transport(),
            100,
        )
        .expect("test step token"),
        1,
        std::num::NonZeroU64::new(1).expect("step"),
        actingcommand_contract::FencedWritePurpose::Business,
    )
}

// The kernel's one input path (`execute_action`): the back key with the step's context reaches
// the input as a key on that frame, under that step's check (its answer comes back); without a
// context it reaches the input as the plain key.
#[test]
fn kb1_fix1_the_kernel_hands_a_key_with_its_steps_context_to_the_input_on_its_frame() {
    let mut input = KeyRecorder::default();
    let witness = step_witness();
    let context = InputExecutionContext {
        geometry: None,
        check: Arc::new(StepCheck),
    };
    let back = PreparedInputAction::Direct(InputAction::Key {
        key: "back".to_owned(),
    });
    execute_action(&mut input, &witness, &back, Some(&context)).expect("the key on its frame");
    execute_action(&mut input, &witness, &back, None).expect("the plain key");
    assert_eq!(
        input.calls,
        [
            (
                "key_in_frame",
                "back".to_owned(),
                Some(Duration::from_millis(STEP_REMAINING_MS))
            ),
            ("key", "back".to_owned(), None),
        ]
    );
}
