// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #406 T4 (plan Q20, Q14; `contracts/resource-declarations.md`, "The Lab's draw"): the
//! touch keys the Lab draws its points with, those of the root configuration it reads, as the
//! OCR loader reads it (an installation's selected configuration, else a single-location root's
//! `actingd.config.json`, Workflow #411 A12). The keys in effect are the configuration's `touch`
//! section over the defaults file the program embeds, as the Runtime of that root starts with
//! them; values an instruction sets while the Runtime runs are not seen. A process outside both
//! an installation and a single-location root has no touch keys, and the Lab draws each box
//! uniformly (`xorshift64_uniform_rect_v1`). A configuration that cannot be read, does not decode
//! or holds a value outside its key's range fails every draw loudly with that failure; nothing
//! is drawn with built-in values.

use crate::ocr::process_root_configuration;
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, Outcome, OutcomeEnvelope};
use actingcommand_contract::{FactScalar, LabError, LabErrorClass, LabResult};
use actingcommand_execution_kernel::TouchSampling;
use actingcommand_runtime_config::{
    DefaultsFile, TouchConfig, TouchFile, find_row, member, parse_value,
};
use serde::Serialize;
use serde_json::Value;

/// The touch keys of the Lab's root configuration, or why they cannot be read.
#[derive(Debug, Clone)]
pub struct LabTouch {
    sampling: Result<Option<TouchSampling>, Outcome>,
}

/// The details of a refused draw.
#[derive(Serialize)]
struct UnreadableDetails {
    outcome: OutcomeEnvelope,
}

impl LabTouch {
    /// The touch keys of this process's root configuration; none outside every root.
    pub fn from_process_installation() -> Self {
        match process_root_configuration() {
            Ok(Some(configuration)) => Self::from_runtime_configuration(&configuration),
            Ok(None) => Self {
                sampling: Ok(Some(TouchSampling {
                    sigma_ratio_milli: 250,
                    max_redraws: 8,
                    candidate_count: 4,
                })),
            },
            Err(cause) => Self {
                sampling: Err(cause),
            },
        }
    }

    /// No touch keys: every box is drawn uniformly.
    pub const fn none() -> Self {
        Self { sampling: Ok(None) }
    }

    /// The touch keys in effect for the Runtime configuration `configuration` (the file's bytes):
    /// its `touch` section over the embedded defaults. A file that does not decode, or whose
    /// `touch` section names a member it does not take, is `config_decode_failed`; a value of the
    /// wrong type or outside its range is `config_value_invalid` for its key.
    pub fn from_runtime_configuration(configuration: &[u8]) -> Self {
        Self {
            sampling: touch_in_effect(configuration),
        }
    }

    /// The keys every draw uses, `None` without a configuration; the failure of a configuration
    /// that cannot be read, as a Lab error whose details carry the outcome.
    pub fn sampling(&self) -> LabResult<Option<TouchSampling>> {
        self.sampling
            .clone()
            .map_err(|outcome| unreadable(&outcome))
    }
}

/// The `touch.sampling` keys in effect for `configuration`.
fn touch_in_effect(configuration: &[u8]) -> Result<Option<TouchSampling>, Outcome> {
    let decode_failed = || Outcome::new(SharedCode::ConfigDecodeFailed);
    let file = serde_json::from_slice::<Value>(configuration).map_err(|_| decode_failed())?;
    if !file.is_object() {
        return Err(decode_failed());
    }
    // The section denies unknown members, as the Runtime's file does.
    if let Some(section) = file.get("touch").filter(|section| !section.is_null()) {
        serde_json::from_value::<TouchFile>(section.clone()).map_err(|_| decode_failed())?;
    }
    let (_, defaults) = DefaultsFile::shipped().map_err(|_| decode_failed())?;
    let value = |key: &str| -> Option<FactScalar> {
        let (row, _) = find_row(key)?;
        let raw = member(&file, key)
            .filter(|value| !value.is_null())
            .or_else(|| member(&defaults, key))?;
        parse_value(row, key, raw).ok()
    };
    let touch = TouchConfig::parse(&value).map_err(|failure| failure.outcome())?;
    Ok(Some(TouchSampling {
        sigma_ratio_milli: touch.sigma_ratio_milli,
        max_redraws: touch.max_redraws,
        candidate_count: touch.candidate_count,
    }))
}

/// A draw refused because the root configuration's touch keys cannot be read.
fn unreadable(outcome: &Outcome) -> LabError {
    let error = LabError::new(
        LabErrorClass::UsageValidation,
        outcome.code().as_str(),
        format!("the touch keys of the Lab's root configuration cannot be read: {outcome}"),
        &["touch_configuration"],
    );
    match serde_json::to_value(UnreadableDetails {
        outcome: outcome.envelope(Detail::Full),
    }) {
        Ok(details) => error.with_details(details),
        Err(_) => error,
    }
}
