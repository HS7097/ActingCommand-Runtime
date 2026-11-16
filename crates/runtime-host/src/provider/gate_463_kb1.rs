// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #463 KB-1 fix round 1 (0.12.69; register §1.447; the coordinator's ruling R5 of 11-16,
//! the review's (h)): the device registry's half of the frame-bound key path. The registry hands
//! the kernel each input wrapped in `DeviceRegistryInputDiagnosticBackend`; a key on its step's
//! frame (the Nemu input's) reaches the wrapped input as `key_in_frame`, with that step's context,
//! and never as the plain `key`. The kernel's half (`execute_action`) is the execution kernel's
//! `session/gate_463_kb1.rs`: no fake provider opens a Nemu session, so no host test drives a key
//! through both. Nothing here opens a backend, sleeps or reads a clock.

use super::*;
use actingcommand_device::{
    DeviceCloseAuthority, DeviceResourceCloseOutcome, InputCheckPhase, InputExecutionContext,
    InputOperationCheck,
};

/// What a step's check answers is left of its deadline; the wrapped input reports it back, so the
/// context it was given is the step's.
const STEP_REMAINING_MS: u64 = 4_631;

/// A step's check that always answers [`STEP_REMAINING_MS`].
struct StepCheck;

impl InputOperationCheck for StepCheck {
    fn check(&self, _phase: InputCheckPhase) -> DeviceResult<Duration> {
        Ok(Duration::from_millis(STEP_REMAINING_MS))
    }
}

/// One call the wrapped input saw: its name, its key and, for a key on a frame, what the context's
/// check answered.
type KeyCall = (&'static str, String, Option<Duration>);

/// The wrapped input: it records each key and refuses every other input, which no test sends.
struct KeyRecorder {
    calls: Arc<Mutex<Vec<KeyCall>>>,
}

impl KeyRecorder {
    fn record(&self, call: KeyCall) -> DeviceResult<()> {
        self.calls.lock().expect("key calls").push(call);
        Ok(())
    }
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
        self.record(("key", key.to_owned(), None))
    }

    fn key_in_frame(
        &mut self,
        _witness: &FencedWrite,
        key: &str,
        context: &InputExecutionContext,
    ) -> DeviceResult<()> {
        let remaining = context.check.check(InputCheckPhase::Continue)?;
        self.record(("key_in_frame", key.to_owned(), Some(remaining)))
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

/// A scheduler-shaped step witness for driving a write method directly.
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

// The registry's input of a Nemu instance, as the kernel holds it (`&mut dyn InputBackend`): the
// back key on its step's frame reaches the wrapped input as a key on that frame, under that step's
// check (its answer comes back), and the plain key as the plain key; neither is turned into the
// other.
#[test]
fn kb1_fix1_the_registry_input_hands_a_key_on_its_frame_to_the_input_as_one() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let mut registry_input = DeviceRegistryInputDiagnosticBackend::new(
        Box::new(KeyRecorder {
            calls: Arc::clone(&calls),
        }),
        TouchBackendChoice::NemuIpc,
    );
    let backend: &mut dyn InputBackend = &mut registry_input;
    let witness = step_witness();
    let context = InputExecutionContext {
        geometry: None,
        check: Arc::new(StepCheck),
    };
    backend
        .key_in_frame(&witness, "back", &context)
        .expect("the key on its frame");
    backend.key(&witness, "back").expect("the plain key");
    assert_eq!(
        *calls.lock().expect("key calls"),
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
