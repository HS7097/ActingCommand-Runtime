// SPDX-License-Identifier: AGPL-3.0-only

// Test plan v2.2 §3.10, E-3 seam EXEC-I4 (inventory-exec §6 I4): the kernel's scripted-frame
// `ContainedTaskRuntime`, for tests only. The device plays a script: `screens[n]` is what it shows
// after `n` inputs, one frame per capture, the segment's last frame repeating (after more inputs
// than segments the last segment goes on). The runtime keeps every input and every trace the
// kernel hands it, samples as the host runtimes do and answers no cancellation.
//
// It has a clock of its own (EXEC-I3): `monotonic_now` is a base instant plus a scripted offset,
// `pause` moves the offset on by its duration and returns at once, and a frame the device takes
// time to deliver moves the offset by that time. No run reads the wall clock through the seam, so
// a window or a budget runs out exactly where a script says, however loaded the machine is.
//
// #385 P-k1a (exec key 91): it answers successive `step_pacing()` calls with a scripted list of
// pacing factors (after the list, its last value; without one, the factor 1000), and it keeps
// every pause the kernel asks for.
//
// #385 P-k2 (exec key 92): every answer may also carry scripted extra wait rounds and a pre-input
// wait (`with_step_extras`; without them none); it counts the extra rounds the kernel starts and
// keeps, in order, the frames' recognitions, the pauses, the effect intents and the inputs.

use super::super::*;
use actingcommand_device::CaptureBackendName;

// #385 P-k2: what the device saw happen, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptedEvent {
    // A frame's recognition completed (`RecognitionCompleted`).
    Recognized,
    // A pause of this length.
    Paused(Duration),
    // An effect intent (`EffectIntent`).
    EffectIntent,
    // An input.
    Input,
}

// The scripted clock starts this far ahead of the wall clock. The recognition pack still compares
// a sampled target's deadline with the wall clock (outside the seam; ledger "E-3 done": mixed
// clocks in recognition-pack are out of E-3's scope). Starting an hour ahead keeps that comparison
// from ever deciding a scripted run, so only the scripted clock does.
const CLOCK_LEAD: Duration = Duration::from_secs(3_600);

// One frame of a script: its RGB8 pixels and how long the device takes to deliver it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScriptedFrame {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    latency: Duration,
}

impl ScriptedFrame {
    // A `width` x `height` frame of `pixels`, row by row, delivered at once.
    pub(crate) fn rgb(width: u32, height: u32, pixels: &[[u8; 3]]) -> Self {
        assert_eq!(
            pixels.len() as u64,
            u64::from(width) * u64::from(height),
            "one colour per pixel"
        );
        Self {
            width,
            height,
            pixels: pixels.concat(),
            latency: Duration::ZERO,
        }
    }

    // The same frame, delivered `latency` after its capture starts.
    pub(crate) fn delivered_after(self, latency: Duration) -> Self {
        Self { latency, ..self }
    }

    fn frame(&self) -> Frame {
        Frame::from_pixels(
            self.width,
            self.height,
            self.pixels.clone(),
            PixelFormat::Rgb8,
            CaptureBackendName::FixtureSimulation,
        )
        .expect("scripted frame")
    }
}

// The Runtime as the kernel sees it, playing a script (see the header).
pub(crate) struct ScriptedRuntime {
    screens: Vec<Vec<ScriptedFrame>>,
    shown: Vec<usize>,
    sample_timeout_retries: u32,
    step_factors: Vec<u32>,
    step_pacings: usize,
    pauses: Vec<Duration>,
    extra_wait_rounds: u32,
    pre_input_wait: Duration,
    extra_rounds_started: u32,
    events: Vec<ScriptedEvent>,
    base: Instant,
    offset: Duration,
    captures: usize,
    inputs: Vec<InputAction>,
    traces: Vec<ContainedTaskTrace>,
}

impl ScriptedRuntime {
    // A device that shows `screens[n]` after `n` inputs; its steps get no observe-only retry.
    pub(crate) fn new(screens: Vec<Vec<ScriptedFrame>>) -> Self {
        assert!(
            !screens.is_empty() && screens.iter().all(|frames| !frames.is_empty()),
            "every segment of a script shows a frame"
        );
        Self {
            shown: vec![0; screens.len()],
            screens,
            sample_timeout_retries: 0,
            step_factors: Vec::new(),
            step_pacings: 0,
            pauses: Vec::new(),
            extra_wait_rounds: 0,
            pre_input_wait: Duration::ZERO,
            extra_rounds_started: 0,
            events: Vec::new(),
            base: Instant::now() + CLOCK_LEAD,
            offset: Duration::ZERO,
            captures: 0,
            inputs: Vec::new(),
            traces: Vec::new(),
        }
    }

    // The same device, giving each step `retries` observe-only retries of a sampling that ran
    // past its window (`step_pacing`, Workflow #386 SR-1).
    pub(crate) fn with_sample_timeout_retries(self, retries: u32) -> Self {
        Self {
            sample_timeout_retries: retries,
            ..self
        }
    }

    // The same device, answering the `n`-th `step_pacing()` call with the pacing factor
    // `factors[n]` (in thousandths), and every later call with the list's last value (#385 P-k).
    pub(crate) fn with_step_factors(self, factors: Vec<u32>) -> Self {
        Self {
            step_factors: factors,
            ..self
        }
    }

    // The same device, whose every `step_pacing()` answer also gives `extra_wait_rounds` extra
    // wait rounds (the observe-only retries growing by as many, as `StepPacing` composes them)
    // and the pre-input wait `pre_input_wait` (#385 P-k2).
    pub(crate) fn with_step_extras(self, extra_wait_rounds: u32, pre_input_wait: Duration) -> Self {
        Self {
            extra_wait_rounds,
            pre_input_wait,
            ..self
        }
    }

    // Every pause the kernel asked for, in order.
    pub(crate) fn pauses(&self) -> &[Duration] {
        &self.pauses
    }

    // How many extra wait rounds the kernel started (#385 P-k2).
    pub(crate) fn extra_rounds_started(&self) -> u32 {
        self.extra_rounds_started
    }

    // The recognitions, pauses, effect intents and inputs, in order (#385 P-k2).
    pub(crate) fn events(&self) -> &[ScriptedEvent] {
        &self.events
    }

    // How many frames the kernel captured.
    pub(crate) fn captures(&self) -> usize {
        self.captures
    }

    // The inputs the kernel made, in order.
    pub(crate) fn inputs(&self) -> &[InputAction] {
        &self.inputs
    }

    // The traces the kernel recorded, in order.
    pub(crate) fn traces(&self) -> &[ContainedTaskTrace] {
        &self.traces
    }
}

impl ContainedTaskRuntime for ScriptedRuntime {
    type Error = &'static str;

    fn capture(&mut self) -> Result<ObservedFrame, Self::Error> {
        let segment = self.inputs.len().min(self.screens.len() - 1);
        let frames = &self.screens[segment];
        let shown = &mut self.shown[segment];
        let scripted = &frames[(*shown).min(frames.len() - 1)];
        *shown += 1;
        self.captures += 1;
        self.offset += scripted.latency;
        Ok(scripted.frame().into())
    }

    fn input(
        &mut self,
        action: InputAction,
        _frame: Option<InputFrameContext>,
    ) -> Result<(), Self::Error> {
        self.inputs.push(action);
        self.events.push(ScriptedEvent::Input);
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        match &trace {
            ContainedTaskTrace::RecognitionCompleted { .. } => {
                self.events.push(ScriptedEvent::Recognized);
            }
            ContainedTaskTrace::EffectIntent { .. } => {
                self.events.push(ScriptedEvent::EffectIntent);
            }
            _ => {}
        }
        self.traces.push(trace);
        Ok(())
    }

    fn sampling_supported(&self) -> bool {
        true
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        let pacing = StepPacing::new(self.sample_timeout_retries);
        let factor = self
            .step_factors
            .get(self.step_pacings)
            .or_else(|| self.step_factors.last())
            .copied();
        self.step_pacings += 1;
        let pacing = match factor {
            Some(factor_milli) => pacing.with_factor_milli(factor_milli),
            None => pacing,
        };
        Ok(pacing
            .with_extra_wait_rounds(self.extra_wait_rounds)
            .with_pre_input_wait(self.pre_input_wait))
    }

    fn observe_extra_wait_round(&mut self) {
        self.extra_rounds_started += 1;
    }

    fn monotonic_now(&mut self) -> Instant {
        self.base + self.offset
    }

    fn pause(&mut self, duration: Duration) {
        self.pauses.push(duration);
        self.events.push(ScriptedEvent::Paused(duration));
        self.offset += duration;
    }
}
