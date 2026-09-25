// SPDX-License-Identifier: AGPL-3.0-only

//! Fixture simulation backends: device-free instances that replay configured frames and
//! accept a bounded number of inputs. Compiled only with the `fixture-backends` feature.

use actingcommand_contract::{BackendOpenEntry, InstanceId};
use actingcommand_device::{
    CaptureBackend, DeviceCloseAuthority, DeviceError, DeviceErrorCategory, DeviceErrorSensitivity,
    DeviceResourceCloseOutcome, DeviceResult, Frame, InputBackend, OpenedBackend,
};
use actingcommand_execution_kernel::{ForegroundApplicationObservation, ResolvedExecutionInstance};
use std::collections::VecDeque;
use std::time::Instant;

/// What a fixture instance replays: its frames, in order, and its input budget.
pub struct FixtureInstanceSpec {
    frames: Vec<Frame>,
    max_inputs: u16,
}

impl FixtureInstanceSpec {
    pub fn new(frames: Vec<Frame>, max_inputs: u16) -> Self {
        Self { frames, max_inputs }
    }
}

pub(super) struct FixtureEntry {
    instance_id: InstanceId,
    frames: Vec<Frame>,
    max_inputs: u16,
}

impl FixtureEntry {
    pub(super) fn new(instance_id: InstanceId, spec: FixtureInstanceSpec) -> Self {
        Self {
            instance_id,
            frames: spec.frames,
            max_inputs: spec.max_inputs,
        }
    }

    pub(super) const fn instance_id(&self) -> InstanceId {
        self.instance_id
    }

    pub(super) fn resolve(&self) -> ResolvedExecutionInstance {
        ResolvedExecutionInstance::fixture_simulation(self.instance_id)
    }

    pub(super) fn open_input(&self) -> OpenedBackend<Box<dyn InputBackend>> {
        OpenedBackend::simulation(
            Box::new(FixtureInputBackend {
                remaining: self.max_inputs,
                closed: false,
            }) as Box<dyn InputBackend>,
            BackendOpenEntry::Input,
        )
    }

    pub(super) fn open_capture(&self) -> DeviceResult<OpenedBackend<Box<dyn CaptureBackend>>> {
        Ok(OpenedBackend::simulation(
            Box::new(FixtureCaptureBackend {
                frames: self
                    .frames
                    .iter()
                    .map(Frame::try_clone)
                    .collect::<DeviceResult<VecDeque<_>>>()?,
            }) as Box<dyn CaptureBackend>,
            BackendOpenEntry::Capture,
        ))
    }

    pub(super) fn control_application() -> DeviceResult<()> {
        Err(DeviceError::fatal(
            "fixture application control is forbidden",
        ))
    }

    /// A fixture has no ADB baseline; the host never asks for one.
    pub(super) fn observe_foreground_application() -> DeviceResult<ForegroundApplicationObservation>
    {
        Err(
            DeviceError::fatal("foreground application observation unsupported by this provider")
                .with_diagnostic(
                    DeviceErrorCategory::Protocol,
                    "application.foreground_unsupported",
                )
                .with_diagnostic_context(
                    "execution_backend_provider",
                    "observe_foreground_application",
                    DeviceErrorSensitivity::Sensitive,
                ),
        )
    }

    pub(super) fn probe_adb_baseline_until(
        deadline: Instant,
        stopped: &dyn Fn() -> bool,
    ) -> DeviceResult<()> {
        if stopped() || Instant::now() >= deadline {
            Err(DeviceError::fatal(
                "ADB baseline stopped or deadline expired",
            ))
        } else {
            Ok(())
        }
    }
}

struct FixtureCaptureBackend {
    frames: VecDeque<Frame>,
}

impl CaptureBackend for FixtureCaptureBackend {
    fn capture(&mut self) -> DeviceResult<Frame> {
        self.frames
            .pop_front()
            .ok_or_else(|| DeviceError::fatal("fixture capture exhausted"))
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        Ok(DeviceResourceCloseOutcome::confirmed(0))
    }
}

struct FixtureInputBackend {
    remaining: u16,
    closed: bool,
}

impl FixtureInputBackend {
    fn consume(&mut self) -> DeviceResult<()> {
        if self.closed || self.remaining == 0 {
            return Err(DeviceError::fatal("fixture input budget exhausted"));
        }
        self.remaining -= 1;
        Ok(())
    }
}

impl InputBackend for FixtureInputBackend {
    fn tap(&mut self, _x: i32, _y: i32) -> DeviceResult<()> {
        self.consume()
    }

    fn long_tap(&mut self, _x: i32, _y: i32, _duration_ms: u64) -> DeviceResult<()> {
        self.consume()
    }

    fn swipe(
        &mut self,
        _x1: i32,
        _y1: i32,
        _x2: i32,
        _y2: i32,
        _duration_ms: u64,
    ) -> DeviceResult<()> {
        self.consume()
    }

    fn key(&mut self, _key: &str) -> DeviceResult<()> {
        self.consume()
    }

    fn text(&mut self, _text: &str) -> DeviceResult<()> {
        self.consume()
    }

    fn reset(&mut self) -> DeviceResult<()> {
        self.consume()
    }

    fn close_once(
        &mut self,
        _authority: DeviceCloseAuthority,
    ) -> DeviceResult<DeviceResourceCloseOutcome> {
        let resource_count = u16::from(!self.closed);
        self.closed = true;
        Ok(DeviceResourceCloseOutcome::confirmed(resource_count))
    }
}
