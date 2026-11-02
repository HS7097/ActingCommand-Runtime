// SPDX-License-Identifier: AGPL-3.0-only
// Adapted from HS7097/codex-goal-lifecycle@c9538cc5 scripts/runtime/src/bridge/security.rs. Copyright (c) 2026 HS7097. MIT: see ../LICENSE-codex-goal-lifecycle.txt.

//! A credential at rest and in memory (Workflow #432; `contracts/agent-forward.md`): Windows DPAPI
//! for the current user, and [`Secret`], bytes that are zeroed when they go away.

use std::io;
use std::ptr::{null, null_mut};
use std::sync::atomic::{Ordering, compiler_fence};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

/// Sets every byte to zero in a way the compiler may not remove as a dead store.
fn zero(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // SAFETY: `byte` is a valid, exclusive reference.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// Bytes that hold a credential. They are zeroed on drop, and neither `Debug` nor `Display` exists,
/// so they cannot reach a message by accident.
pub(crate) struct Secret(Vec<u8>);

impl Secret {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// An empty secret that holds `capacity` bytes before its buffer would have to move.
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self(Vec::with_capacity(capacity))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The buffer, to write into. A write past the capacity leaves a copy behind in freed memory,
    /// so the caller sizes the secret first.
    pub(crate) fn buffer(&mut self) -> &mut Vec<u8> {
        &mut self.0
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        zero(&mut self.0);
    }
}

/// `bytes` encrypted for the current Windows user.
pub(crate) fn protect(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut output = crypt(bytes, false)?;
    Ok(std::mem::take(output.buffer()))
}

/// The bytes of a blob [`protect`] made for the current Windows user. A blob of another user, or
/// one that was changed, fails.
pub(crate) fn unprotect(blob: &[u8]) -> io::Result<Secret> {
    crypt(blob, true)
}

fn crypt(bytes: &[u8], decrypt: bool) -> io::Result<Secret> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| io::ErrorKind::InvalidInput)?,
        // The functions read their input only, through a pointer to a const blob.
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: null_mut(),
    };
    // SAFETY: `input` points at `bytes`, which outlives the call; every optional pointer is null;
    // `output` is valid for the call and receives a LocalAlloc buffer on success.
    let ok = unsafe {
        if decrypt {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if output.pbData.is_null() {
        return Ok(Secret::new(Vec::new()));
    }
    // SAFETY: on success `pbData` points at `cbData` initialised bytes this function now owns; they
    // are copied, zeroed and freed exactly once, and nothing else holds them.
    let result = unsafe {
        let buffer = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let copy = buffer.to_vec();
        zero(buffer);
        LocalFree(output.pbData.cast());
        copy
    };
    Ok(Secret::new(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_round_trips_and_a_tampered_blob_fails() {
        let invented = b"invented-value-for-this-test";
        let mut blob = protect(invented).expect("protect");
        assert!(
            !blob.windows(invented.len()).any(|part| part == invented),
            "the blob does not hold the value in the clear"
        );
        assert_eq!(unprotect(&blob).expect("unprotect").as_bytes(), invented);
        let middle = blob.len() / 2;
        blob[middle] ^= 0xff;
        assert!(unprotect(&blob).is_err(), "a changed blob does not decrypt");
    }
}
