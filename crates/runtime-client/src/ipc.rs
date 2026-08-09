// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeClientError, RuntimeClientResult};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub(crate) const DEFAULT_RUNTIME_MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct ReceiptReadDeadline {
    deadline: Instant,
    timeout_code: &'static str,
}

impl ReceiptReadDeadline {
    pub(crate) fn after(timeout: Duration, timeout_code: &'static str) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            timeout_code,
        }
    }

    #[cfg(test)]
    pub(crate) fn at(deadline: Instant, timeout_code: &'static str) -> Self {
        Self {
            deadline,
            timeout_code,
        }
    }

    fn has_elapsed(self) -> bool {
        Instant::now() >= self.deadline
    }
}

pub(crate) fn exchange<Request, Response>(
    stream: &mut TcpStream,
    request: &Request,
    maximum_frame_bytes: usize,
    receipt_deadline: Option<ReceiptReadDeadline>,
) -> RuntimeClientResult<Response>
where
    Request: Serialize,
    Response: DeserializeOwned,
{
    let body = serde_json::to_vec(request).map_err(|_| {
        RuntimeClientError::fatal("runtime_request_encode_failed", "exchange_runtime_request")
    })?;
    if body.is_empty() || body.len() > maximum_frame_bytes || body.len() > u32::MAX as usize {
        return Err(RuntimeClientError::fatal(
            "runtime_request_frame_invalid",
            "exchange_runtime_request",
        ));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    stream.write_all(&frame).map_err(|_| {
        RuntimeClientError::fatal("runtime_request_write_failed", "exchange_runtime_request")
    })?;
    stream.flush().map_err(|_| {
        RuntimeClientError::fatal("runtime_request_flush_failed", "exchange_runtime_request")
    })?;

    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).map_err(|error| {
        receipt_read_error(error, "runtime_receipt_header_failed", receipt_deadline)
    })?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > maximum_frame_bytes {
        return Err(RuntimeClientError::fatal(
            "runtime_receipt_frame_invalid",
            "exchange_runtime_request",
        ));
    }
    let mut response = vec![0_u8; length];
    stream.read_exact(&mut response).map_err(|error| {
        receipt_read_error(error, "runtime_receipt_read_failed", receipt_deadline)
    })?;
    serde_json::from_slice(&response).map_err(|_| {
        RuntimeClientError::fatal("runtime_receipt_decode_failed", "exchange_runtime_request")
    })
}

fn receipt_read_error(
    error: std::io::Error,
    fallback_code: &'static str,
    receipt_deadline: Option<ReceiptReadDeadline>,
) -> RuntimeClientError {
    let code = match receipt_deadline {
        Some(deadline)
            if matches!(
                error.kind(),
                ErrorKind::TimedOut
                    | ErrorKind::WouldBlock
                    | ErrorKind::UnexpectedEof
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::ConnectionReset
                    | ErrorKind::BrokenPipe
                    | ErrorKind::NotConnected
            ) && deadline.has_elapsed() =>
        {
            deadline.timeout_code
        }
        _ => fallback_code,
    };
    RuntimeClientError::fatal(code, "exchange_runtime_request")
}
