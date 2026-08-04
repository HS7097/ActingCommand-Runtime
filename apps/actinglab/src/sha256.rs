use super::{CliError, CliOutcome};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

pub(super) fn file_sha256(path: &Path) -> CliOutcome<String> {
    let bytes = fs::read(path)
        .map_err(|err| CliError::usage(format!("failed to read {}: {err}", path.display())))?;
    Ok(hex_sha256(&bytes))
}

pub(super) fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
