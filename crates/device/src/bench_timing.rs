// SPDX-License-Identifier: AGPL-3.0-only

//! #402 step 1 (C1): the device layer's timing points for the experiment carrier
//! `tools/capture-bench` (plan v2.1 §3.2 item 1).
//!
//! The points are off unless the cargo feature `bench-timing` is on, and it is off by default.
//! Without the feature [`emit`] never runs its argument and no sink can be installed, so every
//! point is a no-op: the event is not built, no clock is read and nothing is stored. No release
//! binary turns the feature on (`workspace_guards.rs`, the #402 guard). With the feature the
//! carrier installs one process-wide [`BenchSink`] and receives each point as it happens:
//!
//! - Nemu IPC: the worker receives a command; the vendor stdio gate is entered and left (the
//!   gate wait); each of the two native calls of a frame starts and ends (the size probe and the
//!   pixel read apart); the vertical flip; the `Frame` is built (a monotonic instant next to the
//!   wall-clock `captured_at`).
//! - scrcpy: a packet is read (`T_recv`); decoding starts and ends; `to_rgb8` starts and ends;
//!   `wait_fresh` starts and ends; every `meets` decision with its inputs (presentation time,
//!   offset minimum, margin, floor) and its result; the open's forward (scid and local port) and
//!   the server's argument list.
//!
//! The event types are plain data and exist without the feature so the call sites compile the
//! same way in both configurations.

use std::time::{Instant, SystemTime};

/// Whether this build records the timing points.
pub const ENABLED: bool = cfg!(feature = "bench-timing");

/// Where a timing point sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BenchPoint {
    /// The Nemu worker took a command off its channel; the detail names the command.
    NemuCommandReceived,
    /// Before the vendor stdio session resumes (the process stdio gate is requested).
    NemuStdioResumeStart,
    /// The vendor stdio session resumed (the gate is held) or failed to.
    NemuStdioResumeEnd,
    /// The vendor stdio was read back and restored (the gate is released) or failed to.
    NemuStdioRestoreEnd,
    /// The size probe's `nemu_capture_display` call starts.
    NemuNativeProbeStart,
    /// The size probe's native call returned.
    NemuNativeProbeEnd,
    /// The pixel read's `nemu_capture_display` call starts.
    NemuNativeCaptureStart,
    /// The pixel read's native call returned.
    NemuNativeCaptureEnd,
    /// The bottom-up to top-down copy starts.
    NemuFlipStart,
    /// The bottom-up to top-down copy ended.
    NemuFlipEnd,
    /// The Nemu `Frame` was built; the detail carries its `captured_at`.
    NemuFrameBuilt,
    /// A scrcpy media packet was read completely (`T_recv`).
    ScrcpyPacketReceived,
    /// Decoding of a scrcpy packet starts.
    ScrcpyDecodeStart,
    /// Decoding of a scrcpy packet ended.
    ScrcpyDecodeEnd,
    /// A decoded picture starts its conversion to RGB8.
    ScrcpyToRgbStart,
    /// The conversion to RGB8 ended.
    ScrcpyToRgbEnd,
    /// A fresh-frame wait starts.
    ScrcpyWaitFreshStart,
    /// A fresh-frame wait ended.
    ScrcpyWaitFreshEnd,
    /// One `meets` decision.
    ScrcpyMeets,
    /// The open forwarded its socket: the scid and the local port.
    ScrcpyForwarded,
    /// The open starts the server with this argument list.
    ScrcpyServerArguments,
}

/// What a timing point carries besides its time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BenchDetail {
    None,
    /// The Nemu command's name.
    NemuCommand(&'static str),
    /// A native call's return code and the size it reported.
    NativeReturn {
        code: i32,
        width: i32,
        height: i32,
    },
    /// A step's success.
    Succeeded(bool),
    /// The wall-clock time a frame carries.
    FrameBuilt {
        captured_at: SystemTime,
    },
    /// A scrcpy packet: its presentation time, its payload size and whether it is a config
    /// packet.
    Packet {
        device_pts_us: u64,
        bytes: usize,
        config: bool,
    },
    /// A decoded picture's size, `None` when the packet completed no picture.
    Decoded {
        picture: Option<(u32, u32)>,
    },
    /// The end of a fresh-frame wait: the sequence delivered, `None` when it failed.
    WaitFresh {
        delivered_sequence: Option<u64>,
    },
    /// The inputs and the result of one `meets` decision.
    Meets(MeetsInputs),
    /// The open's forward.
    Forwarded {
        scid: u32,
        local_port: u16,
    },
    /// The server's arguments after `adb shell`, the `CLASSPATH=` assignment first.
    ServerArguments {
        scid: u32,
        arguments: Vec<String>,
    },
}

/// The inputs and the result of one scrcpy `meets` decision (`scrcpy.rs`, `SlotState::meets`).
/// Times are microseconds relative to the stream's epoch (the instant its slot was created),
/// signed as the decision computes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetsInputs {
    pub sequence: u64,
    pub device_pts_us: u64,
    pub received_at: Instant,
    pub epoch: Instant,
    /// The smallest arrival-minus-presentation difference in the window; `None` without one.
    pub offset_min_us: Option<i128>,
    pub margin_us: u128,
    pub floor_after: Option<Instant>,
    pub floor_after_sequence: Option<u64>,
    pub met: bool,
}

/// One timing point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchEvent {
    pub point: BenchPoint,
    /// When the point happened (the host's monotonic clock, QueryPerformanceCounter on Windows).
    pub at: Instant,
    /// The Nemu instance the point belongs to, when the point knows it.
    pub nemu_instance: Option<i32>,
    /// The scrcpy stream (its scid) the point belongs to, when the point knows it.
    pub stream_scid: Option<u32>,
    pub detail: BenchDetail,
}

impl BenchEvent {
    /// A point that happens now.
    pub fn now(point: BenchPoint, detail: BenchDetail) -> Self {
        Self {
            point,
            at: Instant::now(),
            nemu_instance: None,
            stream_scid: None,
            detail,
        }
    }

    pub fn for_nemu(mut self, instance: i32) -> Self {
        self.nemu_instance = Some(instance);
        self
    }

    pub fn for_stream(mut self, scid: Option<u32>) -> Self {
        self.stream_scid = scid;
        self
    }
}

/// Receives the timing points. It runs on the thread of the point (a Nemu worker, a scrcpy
/// reader, or the caller), so it must return quickly and never call back into the device layer.
pub trait BenchSink: Send + Sync {
    fn record(&self, event: BenchEvent);
}

#[cfg(feature = "bench-timing")]
mod enabled {
    use super::{BenchEvent, BenchSink};
    use std::cell::Cell;
    use std::sync::{Arc, RwLock};

    static SINK: RwLock<Option<Arc<dyn BenchSink>>> = RwLock::new(None);

    thread_local! {
        static STREAM: Cell<Option<u32>> = const { Cell::new(None) };
    }

    pub fn install(sink: Arc<dyn BenchSink>) {
        match SINK.write() {
            Ok(mut slot) => *slot = Some(sink),
            Err(poisoned) => *poisoned.into_inner() = Some(sink),
        }
    }

    pub fn uninstall() {
        match SINK.write() {
            Ok(mut slot) => *slot = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }

    pub(crate) fn emit(event: impl FnOnce() -> BenchEvent) {
        let sink = match SINK.read() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        if let Some(sink) = sink {
            sink.record(event());
        }
    }

    pub(crate) fn set_stream(scid: Option<u32>) {
        STREAM.with(|stream| stream.set(scid));
    }

    pub(crate) fn stream() -> Option<u32> {
        STREAM.with(Cell::get)
    }
}

/// Installs the process-wide sink (the experiment carrier only); a later call replaces it.
#[cfg(feature = "bench-timing")]
pub fn install(sink: std::sync::Arc<dyn BenchSink>) {
    enabled::install(sink);
}

/// Removes the process-wide sink.
#[cfg(feature = "bench-timing")]
pub fn uninstall() {
    enabled::uninstall();
}

/// Records one point. Without the feature `event` is never called.
#[inline(always)]
pub(crate) fn emit(event: impl FnOnce() -> BenchEvent) {
    #[cfg(feature = "bench-timing")]
    enabled::emit(event);
    #[cfg(not(feature = "bench-timing"))]
    let _ = event;
}

/// Names the scrcpy stream of the calling thread (the stream's reader thread), so the points the
/// reader reaches carry its scid. Without the feature nothing is stored.
#[inline(always)]
pub(crate) fn set_stream(scid: Option<u32>) {
    #[cfg(feature = "bench-timing")]
    enabled::set_stream(scid);
    #[cfg(not(feature = "bench-timing"))]
    let _ = scid;
}

/// The scrcpy stream of the calling thread; always `None` without the feature.
#[inline(always)]
pub(crate) fn stream() -> Option<u32> {
    #[cfg(feature = "bench-timing")]
    {
        enabled::stream()
    }
    #[cfg(not(feature = "bench-timing"))]
    {
        None
    }
}
