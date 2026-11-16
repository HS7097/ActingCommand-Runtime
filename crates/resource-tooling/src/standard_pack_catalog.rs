// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #477 PC-1 (register §1.438; contracts/pack-catalog.md, "Who writes it"): `actinglab
//! standard-pack catalog --root <game root> [--plan]`, the writer of a game root's
//! `pack-catalog.json`, so that nobody edits the file by hand.
//!
//! It builds or brings up to date the file from what the root already declares, by the contract
//! crate's one rule ([`merge_standard_pack_catalog`]): an entry for every package of `bundle.json`;
//! a new entry's display from a leftover `ui.json`, else none; its `consumes` and `produces` the
//! union over the package's rules in `schedule.json`; an entry already there kept as written, its
//! differences from the rules reported, never overwritten silently. With `--plan` it writes nothing
//! and answers the document it would write. Without it, it writes the file atomically (a `.part`
//! beside it, renamed over it) when its bytes differ, and leaves it as it is when they do not. The
//! root is given by its path, a repository's working copy or a scratch copy; one inside an
//! installation is refused (`standard_pack_catalog_root_refused`): an installed root takes the file
//! through the build and the installer's `place`. It reads only the root's `bundle.json`,
//! `schedule.json`, `pack-catalog.json` and `ui.json`, writes only `pack-catalog.json` (and its
//! `.part`) and deletes nothing: a leftover `ui.json` stays, and its warning with it.

use crate::standard_pack_check::{catalog_refused, read_bounded, root_invalid};
use crate::{StandardPackCatalogRequest, StandardPackCatalogResponse, installed_root_containing};
use actingcommand_contract::codes::SharedCode;
use actingcommand_contract::outcome::{Detail, IoOp, Outcome};
use actingcommand_contract::{
    LabError, LabErrorClass, LabResult, STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_CATALOG_FILE,
    STANDARD_PACK_RETIRED_UI_FILE, STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem,
    StandardPackRegistry, StandardPackRootIssue, catalog_display_outcome,
    merge_standard_pack_catalog, parse_standard_pack_schedule, retired_ui_outcome,
};
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The answer's `status` of a plan.
const PLANNED: &str = "planned";
/// The answer's `status` of a line that wrote the file.
const WRITTEN: &str = "written";
/// The answer's `status` of a line that found the file holding the bytes already.
const UNCHANGED: &str = "unchanged";

/// `standard-pack catalog` (module doc).
pub fn write_standard_pack_catalog(
    request: StandardPackCatalogRequest,
) -> LabResult<StandardPackCatalogResponse> {
    let root = std::path::absolute(&request.root)
        .map_err(|error| LabError::usage(format!("{}: {error}", request.root.display())))?;
    if !fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(LabError::usage(format!(
            "--root {} is no directory",
            root.display()
        )));
    }
    let game = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            LabError::usage(format!("--root {} names no game directory", root.display()))
        })?
        .to_owned();
    if let Some(installation) = installed_root_containing(&root)? {
        return Err(refused(
            Outcome::new(SharedCode::StandardPackCatalogRootRefused)
                .path(&root)
                .install_root(&installation),
            format!(
                "{} lies inside the installation {}; standard-pack catalog writes a repository's \
                 working copy or a scratch copy only, and an installed root takes the file \
                 through standard-pack build and place",
                root.display(),
                installation.display()
            ),
        ));
    }
    let bundle_path = root.join(STANDARD_PACK_BUNDLE_FILE);
    let Some(bundle) = read_bounded(&bundle_path)? else {
        return Err(root_invalid(
            &game,
            &bundle_path,
            &StandardPackRootIssue {
                file: STANDARD_PACK_BUNDLE_FILE,
                problem: StandardPackProblem::BundleMissing,
                pointer: String::new(),
            },
        ));
    };
    let registry = StandardPackRegistry::from_bundle_bytes(&bundle, &game)
        .map_err(|issue| root_invalid(&game, &bundle_path, &issue))?;
    let schedule_path = root.join(STANDARD_PACK_SCHEDULE_FILE);
    let schedule = read_bounded(&schedule_path)?
        .map(|bytes| parse_standard_pack_schedule(&bytes))
        .transpose()
        .map_err(|issue| root_invalid(&game, &schedule_path, &issue))?;
    let path = root.join(STANDARD_PACK_CATALOG_FILE);
    let existing = read_bounded(&path)?;
    let retired_path = root.join(STANDARD_PACK_RETIRED_UI_FILE);
    let retired = read_bounded(&retired_path)?;
    // A leftover `ui.json` that is no JSON gives no display; its warning names it either way.
    let retired_document = retired
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
    let merge = merge_standard_pack_catalog(
        &game,
        &registry,
        schedule.as_ref(),
        existing.as_deref(),
        retired_document.as_ref(),
    )
    .map_err(|issues| catalog_refused(&game, &path, issues))?;
    let changed = existing.as_deref() != Some(merge.bytes.as_slice());
    let mut warnings = merge
        .catalog
        .display_issues()
        .iter()
        .map(|issue| catalog_display_outcome(&game, &path, issue))
        .collect::<Vec<_>>();
    warnings.extend(
        merge
            .differences
            .iter()
            .map(|difference| difference.outcome(&game)),
    );
    if retired.is_some() {
        warnings.push(retired_ui_outcome(&game, &retired_path));
    }
    let (status, document) = if request.plan {
        let document = serde_json::from_slice::<Value>(&merge.bytes).map_err(|error| {
            LabError::usage(format!(
                "the document standard-pack catalog makes does not read back: {error}"
            ))
        })?;
        (PLANNED, Some(document))
    } else if changed {
        write_atomically(&path, &merge.bytes)?;
        (WRITTEN, None)
    } else {
        (UNCHANGED, None)
    };
    Ok(StandardPackCatalogResponse {
        status: status.to_owned(),
        root: root.display().to_string(),
        game,
        path: path.display().to_string(),
        changed,
        added: merge.added,
        kept: merge.kept,
        removed: merge.removed,
        dropped: merge.dropped,
        differences: merge.differences,
        display_from_ui: merge.display_from_ui,
        document,
        warnings,
    })
}

/// Writes `bytes` to `path` atomically: to `<path>.part` (made anew, synced), then renamed over
/// `path`. A failure is `standard_pack_catalog_write_failed` naming `path`, the OS error beneath
/// it; the file in place is then as it was.
fn write_atomically(path: &Path, bytes: &[u8]) -> LabResult<()> {
    let part = part_path(path);
    let failed = |error: &io::Error, op: IoOp| {
        refused(
            Outcome::new(SharedCode::StandardPackCatalogWriteFailed)
                .path(path)
                .caused_by(Outcome::from_io_error(error, op).path(&part)),
            format!("{} could not be written: {error}", path.display()),
        )
    };
    fs::File::create(&part)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .map_err(|error| failed(&error, IoOp::Write))?;
    fs::rename(&part, path).map_err(|error| failed(&error, IoOp::Rename))
}

/// `<path>.part`.
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

/// The Lab error of the refusal `outcome`.
fn refused(outcome: Outcome, message: String) -> LabError {
    LabError::new(
        LabErrorClass::UsageValidation,
        outcome.code().as_str(),
        message,
        &[],
    )
    .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}
