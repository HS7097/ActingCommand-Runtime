// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use serde::Serialize;
use std::io::{self, Read, Write};
use std::net::TcpStream;

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

pub(crate) fn write_frame<T: Serialize>(
    stream: &mut TcpStream,
    value: &T,
    maximum_frame_bytes: usize,
) -> RuntimeHostResult<()> {
    let body =
        serde_json::to_vec(value).map_err(|_| protocol_error("runtime_frame_encode_failed"))?;
    if body.is_empty() || body.len() > maximum_frame_bytes || body.len() > u32::MAX as usize {
        return Err(protocol_error("runtime_frame_length_invalid"));
    }
    let mut frame = Vec::with_capacity(body.len() + 4);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
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
