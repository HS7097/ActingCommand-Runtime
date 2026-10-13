// SPDX-License-Identifier: AGPL-3.0-only

//! The defaults file `distribution/windows/actingd.defaults.json` (Workflow #387 model v1.2
//! §5.2): the lowest layer of every key whose `default` is `file`, compiled into the program and
//! shipped beside the example configuration. It is parsed by its own structure, with no
//! top-level required member; a member without parsing code is refused. Each section adds its
//! own member here (D1b ships the mechanism; D1c adds `recovery_ladder` and `host_claims`).

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The defaults file the program embeds; the build ships the same bytes.
pub const SHIPPED_DEFAULTS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../distribution/windows/actingd.defaults.json"
));

/// The defaults file's sections, each as strict as in the configuration file.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsFile {}

/// A defaults file that is not a JSON object of known sections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultsInvalid;

impl DefaultsFile {
    /// Parses a defaults file: its sections, and the JSON the file layer reads defaults from.
    pub fn parse(text: &str) -> Result<(Self, Value), DefaultsInvalid> {
        let value = serde_json::from_str::<Value>(text).map_err(|_| DefaultsInvalid)?;
        if !value.is_object() {
            return Err(DefaultsInvalid);
        }
        let sections = Self::deserialize(&value).map_err(|_| DefaultsInvalid)?;
        Ok((sections, value))
    }

    /// The embedded defaults file, parsed.
    pub fn shipped() -> Result<(Self, Value), DefaultsInvalid> {
        Self::parse(SHIPPED_DEFAULTS)
    }
}

/// `sha256:<hex>` of `bytes`: how `config.selection` names the configuration and defaults files.
pub fn sha256_text(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// The digest of the embedded defaults file (§5.2, §5.7).
pub fn shipped_defaults_sha256() -> String {
    sha256_text(SHIPPED_DEFAULTS.as_bytes())
}
