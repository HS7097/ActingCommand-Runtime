// SPDX-License-Identifier: AGPL-3.0-only

use crate::{VisionFfiError, VisionFfiErrorCode, VisionFfiResult};
pub use actingcommand_contract::{
    PPOCR_DIAGNOSTIC_RESULT_SCHEMA, PPOCR_MAX_BUSINESS_JSON_BYTES, PPOCR_MAX_DIAGNOSTIC_JSON_BYTES,
    PPOCR_MAX_DIAGNOSTIC_NODES, PPOCR_MAX_DIAGNOSTIC_REPORTS, PPOCR_MAX_ENVELOPE_BYTES,
    PPOCR_MAX_NODE_LOG_BYTES, PPOCR_MAX_REPORT_JSON_BYTES, PPOCR_MAX_RESPONSE_BYTES,
    PPOCR_NODE_PLACEMENT_RECORD_TYPE, PpocrCpuAssignedNodeDiagnostic, PpocrDiagnostics,
    PpocrNodePlacementDiagnostic, validate_ppocr_call_diagnostics,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;
use std::io::{self, Write};

struct ResponseWriter {
    bytes: Vec<u8>,
    section_start: usize,
    section_limit: usize,
}

impl ResponseWriter {
    fn begin_section(&mut self, limit: usize) {
        self.section_start = self.bytes.len();
        self.section_limit = limit;
    }
}

impl Write for ResponseWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let next_len = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("PPOCR response length overflow"))?;
        if next_len > PPOCR_MAX_RESPONSE_BYTES || next_len - self.section_start > self.section_limit
        {
            return Err(io::Error::other(
                "PPOCR response section exceeds its byte budget",
            ));
        }
        if next_len > self.bytes.capacity() {
            let growth = self
                .bytes
                .capacity()
                .checked_mul(2)
                .unwrap_or(PPOCR_MAX_RESPONSE_BYTES)
                .clamp(next_len.max(4096), PPOCR_MAX_RESPONSE_BYTES);
            self.bytes
                .try_reserve_exact(growth - self.bytes.len())
                .map_err(|error| {
                    io::Error::other(format!("PPOCR response allocation failed: {error}"))
                })?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialize each business value and each diagnostic object directly into one owned response.
pub fn serialize_ppocr_response<T: Serialize>(
    response: &T,
    diagnostics: &PpocrDiagnostics,
) -> Result<Vec<u8>, String> {
    validate_ppocr_call_diagnostics(diagnostics)?;
    let mut writer = ResponseWriter {
        bytes: Vec::new(),
        section_start: 0,
        section_limit: PPOCR_MAX_ENVELOPE_BYTES,
    };
    writer
        .write_all(b"{\"schema_version\":")
        .map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut writer, PPOCR_DIAGNOSTIC_RESULT_SCHEMA)
        .map_err(|e| e.to_string())?;
    writer
        .write_all(b",\"response\":")
        .map_err(|e| e.to_string())?;
    let header_len = writer.bytes.len();
    writer.begin_section(PPOCR_MAX_BUSINESS_JSON_BYTES);
    serde_json::to_writer(&mut writer, response).map_err(|e| e.to_string())?;
    writer.begin_section(PPOCR_MAX_ENVELOPE_BYTES - header_len);
    writer
        .write_all(b",\"diagnostics\":")
        .map_err(|e| e.to_string())?;
    writer.begin_section(PPOCR_MAX_DIAGNOSTIC_JSON_BYTES);
    serde_json::to_writer(&mut writer, diagnostics).map_err(|e| e.to_string())?;
    writer.begin_section(1);
    writer.write_all(b"}").map_err(|e| e.to_string())?;
    Ok(writer.bytes)
}

#[derive(Deserialize)]
struct ResponseEnvelope<'a> {
    schema_version: Option<String>,
    #[serde(borrow)]
    response: Option<&'a RawValue>,
    #[serde(borrow)]
    diagnostics: Option<&'a RawValue>,
}

pub(crate) fn decode_ppocr_response<O: DeserializeOwned>(
    status: i32,
    bytes: &[u8],
) -> VisionFfiResult<(O, PpocrDiagnostics)> {
    let invalid = |message| {
        VisionFfiError::fatal_with_code(
            VisionFfiErrorCode::InvalidResponse,
            "fastdeploy-ppocr",
            message,
        )
    };
    if status == 0 && bytes.is_empty() {
        return Err(invalid(
            "FFI backend returned an empty response".to_string(),
        ));
    }
    let envelope = serde_json::from_slice::<ResponseEnvelope<'_>>(bytes).ok();
    let (business, diagnostics, typed) = match envelope {
        Some(envelope)
            if envelope.response.is_some()
                || envelope.diagnostics.is_some()
                || envelope.schema_version.as_deref() == Some(PPOCR_DIAGNOSTIC_RESULT_SCHEMA) =>
        {
            let raw = envelope
                .diagnostics
                .ok_or_else(|| invalid("PPOCR diagnostic array missing".to_string()))?;
            if raw.get().len() > PPOCR_MAX_DIAGNOSTIC_JSON_BYTES {
                return Err(invalid("PPOCR diagnostic JSON exceeds 224 MiB".to_string()));
            }
            let diagnostics: PpocrDiagnostics = serde_json::from_str(raw.get())
                .map_err(|error| invalid(format!("invalid PPOCR diagnostic JSON: {error}")))?;
            validate_ppocr_call_diagnostics(&diagnostics)
                .map_err(|message| invalid(message).with_ppocr_diagnostics(diagnostics.clone()))?;
            if envelope.schema_version.as_deref() != Some(PPOCR_DIAGNOSTIC_RESULT_SCHEMA) {
                return Err(
                    invalid("PPOCR diagnostic envelope schema mismatch".to_string())
                        .with_ppocr_diagnostics(diagnostics),
                );
            }
            let business = envelope.response.ok_or_else(|| {
                invalid("PPOCR business response missing".to_string())
                    .with_ppocr_diagnostics(diagnostics.clone())
            })?;
            let envelope_len = bytes
                .len()
                .checked_sub(business.get().len())
                .and_then(|len| len.checked_sub(raw.get().len()))
                .ok_or_else(|| {
                    invalid("PPOCR envelope size underflow".to_string())
                        .with_ppocr_diagnostics(diagnostics.clone())
                })?;
            if business.get().len() > PPOCR_MAX_BUSINESS_JSON_BYTES
                || envelope_len > PPOCR_MAX_ENVELOPE_BYTES
            {
                return Err(
                    invalid("PPOCR business or envelope byte budget exceeded".to_string())
                        .with_ppocr_diagnostics(diagnostics),
                );
            }
            (business.get().as_bytes(), diagnostics, true)
        }
        _ => {
            if bytes.len() > PPOCR_MAX_BUSINESS_JSON_BYTES {
                return Err(invalid(
                    "PPOCR business response exceeds 128 MiB".to_string(),
                ));
            }
            (bytes, Vec::new(), false)
        }
    };
    if status != 0 {
        let message = if typed {
            serde_json::from_slice::<String>(business)
                .unwrap_or_else(|_| String::from_utf8_lossy(business).into_owned())
        } else {
            String::from_utf8_lossy(business).into_owned()
        };
        let code = match status {
            2 => VisionFfiErrorCode::ProviderPanic,
            3 => VisionFfiErrorCode::Timeout,
            _ => VisionFfiErrorCode::ProviderFailure,
        };
        return Err(VisionFfiError::fatal_with_code(
            code,
            "fastdeploy-ppocr",
            format!("FFI backend returned status {status}: {message}"),
        )
        .with_ppocr_diagnostics(diagnostics));
    }
    let response = serde_json::from_slice(business).map_err(|error| {
        invalid(format!("failed to parse FFI response JSON: {error}"))
            .with_ppocr_diagnostics(diagnostics.clone())
    })?;
    Ok((response, diagnostics))
}
