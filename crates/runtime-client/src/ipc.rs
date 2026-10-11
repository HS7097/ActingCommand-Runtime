// SPDX-License-Identifier: AGPL-3.0-only

use crate::codes::{ClientCode, ClientLocation};
use crate::{RuntimeClientError, RuntimeClientResult};
use actingcommand_contract::RuntimeRequest;
use actingcommand_contract::outcome::Code;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

#[cfg(feature = "test-observation")]
use crate::test_observation::{
    ObservationOperation, ObservationOutcome, ObservationStage, ObservationThreadRole,
    record_active,
};

pub(crate) const DEFAULT_RUNTIME_MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct ReceiptReadDeadline {
    deadline: Instant,
    timeout_code: Code,
}

impl ReceiptReadDeadline {
    pub(crate) fn after(timeout: Duration, timeout_code: impl Into<Code>) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            timeout_code: timeout_code.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn at(deadline: Instant, timeout_code: impl Into<Code>) -> Self {
        Self {
            deadline,
            timeout_code: timeout_code.into(),
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
    _observation_request: Option<&RuntimeRequest>,
    maximum_reply_bytes: Option<usize>,
) -> RuntimeClientResult<Response>
where
    Request: Serialize,
    Response: DeserializeOwned,
{
    let body = serde_json::to_vec(request).map_err(|_| {
        RuntimeClientError::fatal(
            ClientCode::RuntimeRequestEncodeFailed,
            ClientLocation::ExchangeRuntimeRequest.location(),
        )
    })?;
    if body.is_empty() || body.len() > maximum_frame_bytes || body.len() > u32::MAX as usize {
        return Err(RuntimeClientError::fatal(
            ClientCode::RuntimeRequestFrameInvalid,
            ClientLocation::ExchangeRuntimeRequest.location(),
        ));
    }
    let mut frame = Vec::with_capacity(4 + body.len());
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&body);
    #[cfg(feature = "test-observation")]
    record_active(
        ObservationStage::ClientRequestWriteStart,
        ObservationOperation::Other,
        ObservationThreadRole::Client,
        ObservationOutcome::Started,
        _observation_request,
        None,
        None,
    );
    if stream.write_all(&frame).is_err() {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientRequestWriteResult,
            ObservationOperation::Other,
            ObservationThreadRole::Client,
            ObservationOutcome::Failure,
            _observation_request,
            None,
            None,
        );
        return Err(RuntimeClientError::fatal(
            ClientCode::RuntimeRequestWriteFailed,
            ClientLocation::ExchangeRuntimeRequest.location(),
        ));
    }
    if stream.flush().is_err() {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ClientRequestWriteResult,
            ObservationOperation::Other,
            ObservationThreadRole::Client,
            ObservationOutcome::Failure,
            _observation_request,
            None,
            None,
        );
        return Err(RuntimeClientError::fatal(
            ClientCode::RuntimeRequestFlushFailed,
            ClientLocation::ExchangeRuntimeRequest.location(),
        ));
    }
    #[cfg(feature = "test-observation")]
    record_active(
        ObservationStage::ClientRequestWriteResult,
        ObservationOperation::Other,
        ObservationThreadRole::Client,
        ObservationOutcome::Success,
        _observation_request,
        None,
        None,
    );

    let mut header = [0_u8; 4];
    #[cfg(feature = "test-observation")]
    record_active(
        ObservationStage::ReceiptHeaderWait,
        ObservationOperation::Other,
        ObservationThreadRole::Client,
        ObservationOutcome::Started,
        _observation_request,
        None,
        None,
    );
    if let Err(error) = stream.read_exact(&mut header) {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ReceiptHeaderResult,
            ObservationOperation::Other,
            ObservationThreadRole::Client,
            observation_error_outcome(&error),
            _observation_request,
            None,
            None,
        );
        return Err(receipt_read_error(
            error,
            ClientCode::RuntimeReceiptHeaderFailed,
            receipt_deadline,
        ));
    }
    #[cfg(feature = "test-observation")]
    record_active(
        ObservationStage::ReceiptHeaderResult,
        ObservationOperation::Other,
        ObservationThreadRole::Client,
        ObservationOutcome::Success,
        _observation_request,
        None,
        None,
    );
    let length = u32::from_be_bytes(header) as usize;
    let response_limit = maximum_reply_bytes
        .unwrap_or(maximum_frame_bytes)
        .min(maximum_frame_bytes);
    if length == 0 || length > response_limit {
        return Err(RuntimeClientError::fatal(
            ClientCode::RuntimeReceiptFrameInvalid,
            ClientLocation::ExchangeRuntimeRequest.location(),
        ));
    }
    let mut response = vec![0_u8; length];
    #[cfg(feature = "test-observation")]
    record_active(
        ObservationStage::ReceiptBodyWait,
        ObservationOperation::Other,
        ObservationThreadRole::Client,
        ObservationOutcome::Started,
        _observation_request,
        None,
        None,
    );
    if let Err(error) = stream.read_exact(&mut response) {
        #[cfg(feature = "test-observation")]
        record_active(
            ObservationStage::ReceiptBodyResult,
            ObservationOperation::Other,
            ObservationThreadRole::Client,
            observation_error_outcome(&error),
            _observation_request,
            None,
            None,
        );
        return Err(receipt_read_error(
            error,
            ClientCode::RuntimeReceiptReadFailed,
            receipt_deadline,
        ));
    }
    #[cfg(feature = "test-observation")]
    record_active(
        ObservationStage::ReceiptBodyResult,
        ObservationOperation::Other,
        ObservationThreadRole::Client,
        ObservationOutcome::Success,
        _observation_request,
        None,
        None,
    );
    serde_json::from_slice(&response).map_err(|_| {
        RuntimeClientError::fatal(
            ClientCode::RuntimeReceiptDecodeFailed,
            ClientLocation::ExchangeRuntimeRequest.location(),
        )
    })
}

#[cfg(feature = "test-observation")]
fn observation_error_outcome(error: &std::io::Error) -> ObservationOutcome {
    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
        ObservationOutcome::Timeout
    } else {
        ObservationOutcome::Failure
    }
}

fn receipt_read_error(
    error: std::io::Error,
    fallback_code: impl Into<Code>,
    receipt_deadline: Option<ReceiptReadDeadline>,
) -> RuntimeClientError {
    let fallback_code = fallback_code.into();
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
    let failure =
        RuntimeClientError::fatal(code, ClientLocation::ExchangeRuntimeRequest.location());
    if fallback_code == ClientCode::RuntimeReceiptHeaderFailed.code() {
        failure.with_receipt_header_io(&error)
    } else {
        failure
    }
}
