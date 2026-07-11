// SPDX-License-Identifier: AGPL-3.0-only

use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::RuntimeErrorCode;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn unix_ms_now() -> RuntimeHostResult<u64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            RuntimeHostError::fatal(
                "system_clock_invalid",
                "read_system_clock",
                RuntimeErrorCode::RuntimeFatal,
            )
        })?
        .as_millis();
    u64::try_from(value).map_err(|_| {
        RuntimeHostError::fatal(
            "system_clock_overflow",
            "read_system_clock",
            RuntimeErrorCode::RuntimeFatal,
        )
    })
}
