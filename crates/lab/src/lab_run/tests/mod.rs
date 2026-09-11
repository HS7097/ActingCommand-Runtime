// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use actingcommand_execution_kernel::canonical_page_anchor;
use actingcommand_recognition_pack::TargetKind;
use serde_json::json;
use std::fs::{self, File};
use std::io::Write;
use tempfile::TempDir;
use zip::ZipWriter;
use zip::write::FileOptions;

include!("bundle_and_actions.rs");
include!("guards_and_recovery.rs");
include!("context_and_output.rs");
include!("fixtures.rs");
