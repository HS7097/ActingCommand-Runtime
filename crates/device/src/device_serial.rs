// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-1a (model-398 v1.1 §3.5 rule 1): an adb device serial that shows only its
//! label. `Debug` and `Display` print the label, there is no `Serialize`, and only
//! [`DeviceSerial::adb_arg`] hands the raw value out, for the adb command line.

use std::fmt;

/// The label prefix of a target addressed by its adb serial (model-398 §3.4).
const HANDSET_LABEL_PREFIX: &str = "handset:";

/// An adb device serial with the label people, logs and records see instead of it.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceSerial {
    raw: String,
    label: String,
}

impl DeviceSerial {
    pub fn new(raw: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            raw: raw.into(),
            label: label.into(),
        }
    }

    /// The serial of the instance `alias`, labelled `handset:<alias>`.
    pub fn handset(raw: impl Into<String>, alias: &str) -> Self {
        Self::new(raw, handset_label(alias))
    }

    /// The raw serial, for the adb command line only.
    pub fn adb_arg(&self) -> &str {
        &self.raw
    }

    /// The label shown wherever the serial would otherwise appear.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl fmt::Debug for DeviceSerial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DeviceSerial")
            .field(&self.label)
            .finish()
    }
}

impl fmt::Display for DeviceSerial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label)
    }
}

/// The label of the instance `alias` addressed by an adb serial: `handset:<alias>`.
pub fn handset_label(alias: &str) -> String {
    format!("{HANDSET_LABEL_PREFIX}{alias}")
}
