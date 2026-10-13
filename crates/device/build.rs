// SPDX-License-Identifier: AGPL-3.0-only

//! #402 RD-3: the pinned scrcpy server.
//!
//! The pin is `components."scrcpy-server-5.0.1"` in
//! `scripts/windows-tools/windows-tool-sources.v1.json`. Its version and SHA-256 become the
//! crate's `SCRCPY_SERVER_VERSION` and `SCRCPY_SERVER_SHA256`. The Windows exact-SHA build
//! fetches the official release asset, checks its size and SHA-256 against the same pin, and
//! names the verified file in `ACTINGCOMMAND_SCRCPY_SERVER` for the build; this script checks
//! the file against the pin again and embeds it. Without the variable (CI test jobs, local
//! development builds) nothing is embedded and the scrcpy backend refuses to open
//! (`scrcpy_server_digest_mismatch`). A named file that does not match fails the build.

use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::path::PathBuf;

const PIN_FILE: &str = "../../scripts/windows-tools/windows-tool-sources.v1.json";
const COMPONENT: &str = "scrcpy-server-5.0.1";
const SERVER_VARIABLE: &str = "ACTINGCOMMAND_SCRCPY_SERVER";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let pin_path = manifest_dir.join(PIN_FILE);
    println!("cargo:rerun-if-changed={}", pin_path.display());
    println!("cargo:rerun-if-env-changed={SERVER_VARIABLE}");
    let pin_text = fs::read_to_string(&pin_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", pin_path.display()));
    let pin: serde_json::Value = serde_json::from_str(&pin_text)
        .unwrap_or_else(|error| panic!("parse {}: {error}", pin_path.display()));
    let component = &pin["components"][COMPONENT];
    let version = component["version"]
        .as_str()
        .unwrap_or_else(|| panic!("{COMPONENT} has no version"));
    let artifacts = component["artifacts"]
        .as_array()
        .unwrap_or_else(|| panic!("{COMPONENT} has no artifacts"));
    let [artifact] = artifacts.as_slice() else {
        panic!("{COMPONENT} must pin exactly one artifact");
    };
    let size = artifact["size"]
        .as_u64()
        .unwrap_or_else(|| panic!("{COMPONENT} has no size"));
    let sha256 = artifact["sha256"]
        .as_str()
        .unwrap_or_else(|| panic!("{COMPONENT} has no sha256"));
    assert!(
        sha256.len() == 64
            && sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{COMPONENT} sha256 is not 64 lowercase hex digits"
    );
    assert!(
        !version.is_empty()
            && version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.'),
        "{COMPONENT} version is not dotted digits"
    );
    println!("cargo:rustc-env=ACTINGCOMMAND_SCRCPY_SERVER_VERSION={version}");
    println!("cargo:rustc-env=ACTINGCOMMAND_SCRCPY_SERVER_SHA256={sha256}");

    let embedded = match env::var_os(SERVER_VARIABLE) {
        None => Vec::new(),
        Some(path) => {
            let path = PathBuf::from(path);
            println!("cargo:rerun-if-changed={}", path.display());
            let bytes =
                fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            assert!(
                bytes.len() as u64 == size,
                "{SERVER_VARIABLE} names {} bytes, the pin says {size}",
                bytes.len()
            );
            const HEX: &[u8; 16] = b"0123456789abcdef";
            let mut actual = String::with_capacity(64);
            for byte in Sha256::digest(&bytes) {
                actual.push(char::from(HEX[usize::from(byte >> 4)]));
                actual.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
            assert!(
                actual == sha256,
                "{SERVER_VARIABLE} has SHA-256 {actual}, the pin says {sha256}"
            );
            bytes
        }
    };
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("scrcpy-server.bin");
    fs::write(&out, embedded).unwrap_or_else(|error| panic!("write {}: {error}", out.display()));
}
