// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #398 RD-1a (model-398 v1.1 §3.5): an adb device serial never leaves the Runtime.
//!
//! - A [`DeviceSerial`] shows only its label: `Debug` and `Display` print the label, there is no
//!   `Serialize`, and only [`DeviceSerial::adb_arg`] hands the raw value out, for the adb command
//!   line (rule 1).
//! - Making a `DeviceSerial` binds its serial for the life of the process. Text the device layer
//!   keeps or prints (adb stdout and stderr, error text) has every bound serial replaced by
//!   `[device:<label>]` (rules 2 and 4): the adb server is shared, so one instance's adb output
//!   can name another instance's device.

use std::borrow::Cow;
use std::fmt;
use std::sync::{PoisonError, RwLock};

/// The label prefix of a target addressed by its adb serial (model-398 §3.4).
const HANDSET_LABEL_PREFIX: &str = "handset:";

/// Every serial bound in this process, with its label, in binding order.
static BOUND_SERIALS: RwLock<Vec<(String, String)>> = RwLock::new(Vec::new());

/// An adb device serial with the label people, logs and records see instead of it.
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceSerial {
    raw: String,
    label: String,
}

impl DeviceSerial {
    /// Binds `raw` under `label` for this process. An empty serial binds nothing (configuration
    /// refuses it before this point); binding a serial again takes the new label.
    pub fn new(raw: impl Into<String>, label: impl Into<String>) -> Self {
        let serial = Self {
            raw: raw.into(),
            label: label.into(),
        };
        if !serial.raw.is_empty() {
            let mut bound = BOUND_SERIALS
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            match bound.iter_mut().find(|(raw, _)| *raw == serial.raw) {
                Some((_, label)) => label.clone_from(&serial.label),
                None => bound.push((serial.raw.clone(), serial.label.clone())),
            }
        }
        serial
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

/// The label `raw` is bound under, when it is a bound serial.
pub(crate) fn bound_label(raw: &str) -> Option<String> {
    BOUND_SERIALS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(bound, _)| bound == raw)
        .map(|(_, label)| label.clone())
}

/// `text` with every bound serial replaced by `[device:<label>]`; borrowed when it names none.
pub(crate) fn redact_bound_serials(text: &str) -> Cow<'_, str> {
    let bound = BOUND_SERIALS.read().unwrap_or_else(PoisonError::into_inner);
    let mut redacted = Cow::Borrowed(text);
    for (raw, label) in bound.iter() {
        if redacted.contains(raw.as_str()) {
            let token = format!("[device:{label}]");
            redacted = Cow::Owned(redacted.replace(raw.as_str(), &token));
        }
    }
    redacted
}

/// Whether `text` names a bound serial.
pub(crate) fn names_bound_serial(text: &str) -> bool {
    matches!(redact_bound_serials(text), Cow::Owned(_))
}

/// [`redact_bound_serials`] for text the caller owns.
pub(crate) fn redact_owned(text: String) -> String {
    let redacted = match redact_bound_serials(&text) {
        Cow::Borrowed(_) => None,
        Cow::Owned(redacted) => Some(redacted),
    };
    redacted.unwrap_or(text)
}
