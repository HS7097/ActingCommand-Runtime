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

use super::super::*;
use actingcommand_device::CaptureBackendName;

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
        Ok(())
    }

    fn record(&mut self, trace: ContainedTaskTrace) -> Result<(), Self::Error> {
        self.traces.push(trace);
        Ok(())
    }

    fn sampling_supported(&self) -> bool {
        true
    }

    fn step_pacing(&mut self) -> Result<StepPacing, Self::Error> {
        Ok(StepPacing::new(self.sample_timeout_retries))
    }

    fn monotonic_now(&mut self) -> Instant {
        self.base + self.offset
    }

    fn pause(&mut self, duration: Duration) {
        self.offset += duration;
    }
}
