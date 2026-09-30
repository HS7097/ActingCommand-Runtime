// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use serde::Serialize;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_RUNTIME_MAX_FRAME_BYTES: usize = 1024 * 1024;

pub(crate) enum FrameRead {
    Data(Vec<u8>),
    Idle,
    Closed,
}

pub(crate) fn read_frame(
    stream: &mut TcpStream,
    maximum_frame_bytes: usize,
) -> RuntimeHostResult<FrameRead> {
    let mut header = [0_u8; 4];
    match read_exact_state(stream, &mut header, true)? {
        ExactRead::Complete => {}
        ExactRead::Idle => return Ok(FrameRead::Idle),
        ExactRead::Closed => return Ok(FrameRead::Closed),
    }
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > maximum_frame_bytes {
        return Err(protocol_error("runtime_frame_length_invalid"));
    }
    let mut body = vec![0_u8; length];
    if !matches!(
        read_exact_state(stream, &mut body, false)?,
        ExactRead::Complete
    ) {
        return Err(protocol_error("runtime_frame_truncated"));
    }
    Ok(FrameRead::Data(body))
}

/// Host idle wait between request frames: after each frame the wait only yields for
/// IDLE_YIELD_WINDOW (back-to-back requests are not delayed), then sleeps from IDLE_POLL_MIN,
/// doubling up to IDLE_POLL_MAX. No blocking receive runs while idle, so none can time out.
const IDLE_YIELD_WINDOW: Duration = Duration::from_micros(500);
const IDLE_POLL_MIN: Duration = Duration::from_millis(1);
const IDLE_POLL_MAX: Duration = Duration::from_millis(5);

pub(crate) enum FrameStart {
    Ready,
    Closed,
    Stopped,
}

/// Host side, between frames. Returns Ready once the next frame's first byte is readable (the
/// socket is blocking again, so the read timeout bounds every read of that frame), Closed when
/// the peer closed, or Stopped when `stop_requested` reports true while no byte is waiting.
/// Windows accept() inherits the listener's non-blocking mode and Linux does not; the mode is
/// set explicitly for every frame, so both platforms run this path.
pub(crate) fn wait_frame_start(
    stream: &TcpStream,
    stop_requested: impl Fn() -> bool,
) -> RuntimeHostResult<FrameStart> {
    stream
        .set_nonblocking(true)
        .map_err(|_| protocol_error("runtime_socket_mode_failed"))?;
    let idle_since = Instant::now();
    let mut delay = IDLE_POLL_MIN;
    loop {
        match stream.peek(&mut [0_u8; 1]) {
            Ok(0) => return Ok(FrameStart::Closed),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if stop_requested() {
                    return Ok(FrameStart::Stopped);
                }
                if idle_since.elapsed() < IDLE_YIELD_WINDOW {
                    thread::yield_now();
                } else {
                    thread::sleep(delay);
                    delay = delay.saturating_mul(2).min(IDLE_POLL_MAX);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(protocol_error("runtime_frame_read_failed")),
        }
    }
    stream
        .set_nonblocking(false)
        .map_err(|_| protocol_error("runtime_socket_mode_failed"))?;
    Ok(FrameStart::Ready)
}

/// Host side, after `wait_frame_start` returned Ready: the first byte was readable, so an
/// empty header read is a frame timeout, never idleness.
pub(crate) fn read_started_frame(
    stream: &mut TcpStream,
    maximum_frame_bytes: usize,
) -> RuntimeHostResult<FrameRead> {
    match read_frame(stream, maximum_frame_bytes)? {
        FrameRead::Idle => Err(protocol_error("runtime_frame_timeout")),
        read => Ok(read),
    }
}

pub(crate) fn write_frame<T: Serialize>(
    stream: &mut TcpStream,
    value: &T,
    maximum_frame_bytes: usize,
) -> RuntimeHostResult<()> {
    let body =
        serde_json::to_vec(value).map_err(|_| protocol_error("runtime_frame_encode_failed"))?;
    write_encoded_frame(stream, &body, maximum_frame_bytes)
}

pub(crate) fn write_encoded_frame(
    stream: &mut TcpStream,
    body: &[u8],
    maximum_frame_bytes: usize,
) -> RuntimeHostResult<()> {
    if body.is_empty() || body.len() > maximum_frame_bytes || body.len() > u32::MAX as usize {
        return Err(protocol_error("runtime_frame_length_invalid"));
    }
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    stream
        .write_all(&frame)
        .and_then(|()| stream.flush())
        .map_err(|_| protocol_error("runtime_frame_write_failed"))
}

enum ExactRead {
    Complete,
    Idle,
    Closed,
}

fn read_exact_state(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    idle_allowed: bool,
) -> RuntimeHostResult<ExactRead> {
    let mut offset = 0;
    while offset < buffer.len() {
        match stream.read(&mut buffer[offset..]) {
            Ok(0) if offset == 0 => return Ok(ExactRead::Closed),
            Ok(0) => return Err(protocol_error("runtime_frame_truncated")),
            Ok(read) => offset += read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && offset == 0
                    && idle_allowed =>
            {
                return Ok(ExactRead::Idle);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(protocol_error("runtime_frame_timeout"));
            }
            Err(_) => return Err(protocol_error("runtime_frame_read_failed")),
        }
    }
    Ok(ExactRead::Complete)
}

fn protocol_error(code: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, "runtime_local_ipc", RuntimeErrorCode::ProtocolInvalid)
}
