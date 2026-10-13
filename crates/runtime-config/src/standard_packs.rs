// SPDX-License-Identifier: AGPL-3.0-only

//! The `standard_packs` section (Workflow #373 NAV-6, model M-373 v1.1 §4.3): `root`, the
//! directory that holds one root per game, `<root>/<game>/`, written once by the installer. A
//! relative path resolves against the configuration file's directory (actingd's assembly); an
//! empty one is refused. Without the section no standard-pack root is read. Workflow #387 D1c
//! moves the section here from actingd's own configuration (NAV-6 deviation 5): a key the
//! start-up assembly reads once, reported in `config.parameters.standard_packs` as written.

use crate::table::{
    DefaultSource, FilePath, KeyName, KeyRange, KeyRow, NotSettable, Settable, ValueType,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The directory of the installed standard packages' game roots, as the file writes it.
pub const ROOT: &str = "standard_packs.root";

/// The section as the configuration file writes it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StandardPacksFile {
    pub root: PathBuf,
}

pub(crate) static ROWS: &[KeyRow] = &[KeyRow {
    name: KeyName::Exact(ROOT),
    file_path: FilePath::Key,
    value_type: ValueType::String,
    range: KeyRange::Any,
    // No default: without the key no root is read.
    default: DefaultSource::None,
    settable: Settable::No(NotSettable::ReadAtAssembly),
    reader: "NAV-6",
    since: "0.12.0",
}];
