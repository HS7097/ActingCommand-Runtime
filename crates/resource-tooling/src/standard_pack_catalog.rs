// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #477 PC-1 (register §1.438; contracts/pack-catalog.md, "Who writes it"): `actinglab
//! standard-pack catalog --root <root> [--plan]`, the writer of a game root's `pack-catalog.json`,
//! so that nobody edits the file by hand.
//!
//! Fix round 1 (the independent review's first point): `--root` takes either of two directories.
//!
//! - **A resource repository's checkout**: a directory that holds `applications.json` and no
//!   `bundle.json`, as every resource repository's checkout does (its `bundle.json` is made at build
//!   time). Its game is the one `applications.json` names, and its registry is the `bundle.json`
//!   the build's own function, `package_bundle`, makes of `applications.json` and the directories
//!   of `packs/` (`checkout_registry`): each directory is copied, following links as the build's
//!   copy does, into a scratch directory of the system's temporary folder, which is removed again
//!   before the line answers. The file is written at the checkout's root, where the build carries
//!   it from.
//! - **A built game root** outside an installation: a directory that holds `bundle.json`, named as
//!   its game (the form of the first round, kept: it costs nothing).
//!
//! Either way it builds or brings up to date the file from what the root already declares, by the
//! contract crate's one rule ([`merge_standard_pack_catalog`]): an entry for every package of the
//! registry; a new entry's display from a leftover `ui.json`, else none; its `consumes` and
//! `produces` the union over the package's rules in `schedule.json`; an entry already there kept as
//! written, its differences from the rules reported, never overwritten silently; what the root no
//! longer declares left out and warned of (`standard_pack_catalog_left_out`, fix round 1). With
//! `--plan` it writes nothing and answers the document it would write. Without it, it writes the
//! file atomically (a `.part` beside it, renamed over it) when its bytes differ, and leaves it as it
//! is when they do not; a write that fails is `standard_pack_catalog_write_failed`, of the class
//! device or instance (fix round 1). A root inside an installation is refused
//! (`standard_pack_catalog_root_refused`): an installed root takes the file through the build and
//! the installer's `place`. Of the root it reads `bundle.json` (or `applications.json` and
//! `packs/`), `schedule.json`, `pack-catalog.json` and `ui.json`; in it, it writes only
//! `pack-catalog.json` (and its `.part`) and deletes nothing: a leftover `ui.json` stays, and its
//! warning with it.
//!
//! Workflow #477 PC-2 (register §1.462): `--refresh <package_id>` (repeatable) brings the named
//! entries' business part up to date from their rules, their display and their place kept, and
//! `--displays <file>` (an absolute path) replaces the named entries' displays whole by a display
//! file's, both by the same rule ([`merge_standard_pack_catalog`] with its `CatalogEdits`). A
//! package `--refresh` names that the registry does not hold, or names twice, is
//! `standard_pack_catalog_refresh_refused`; a display file the catalog's own checks refuse is
//! `standard_pack_catalog_display_refused`; either before anything is written. The answer gains
//! `refreshed` and `display_written`, each only when its option is given, so that the line without
//! them answers as before, byte for byte. Fix round 1: the display file carries its
//! `schema_version` and may carry groups, which the line adds (`groups_added`, with `--displays`
//! only); a `--refresh` id an answer could not name (over 256 bytes, or with a control character)
//! is a usage error before the root is read, its value not echoed.
//!
//! Workflow #477 CV-1 (register §1.507): `--schema-version 2` writes version 2 (a file of version 2
//! keeps it without the flag), converting a file of version 1, each entry's business part then
//! its rules' (`converted`); `--values <file>` (an absolute path) replaces the named parts' value
//! and marks, refused as the display file is (`standard_pack_catalog_display_refused`, naming the
//! values file), and a usage error on a line that writes version 1. The answer gains `converted`
//! and `values_written`, each only with its option.
//!
//! Workflow #477 CV-2 (register §1.508; the coordinator's ruling 7): `--cut`
//! ([`cut_standard_pack_catalog`]) cuts a root over to its version 2 file: it composes the root as
//! it stands, makes in memory the `schedule.json` that names `catalog_business` and writes none of
//! its rules' business members, composes that with the file by the same composer, and writes it
//! only when the two compositions are byte for byte the same (the rules digest, the catalog hash,
//! who wrote each value) and no rule differs from the part that would stand for it; otherwise it
//! writes nothing and is refused with every difference, rule by rule, by the catalog's own words.
//! Fix round 1 (the coordinator's ruling): on a switched root the writer never takes a business part
//! from the rules, which write none: `--refresh`, a missing file and a file of version 1 are refused
//! `validation_failed` ([`switched_refused`]: the file is restored, not rebuilt), and a new entry
//! gets its display only.

use crate::standard_pack_check::{catalog_refused, read_bounded, root_invalid};
use crate::standard_pack_schedule::compose_per_server;
use crate::{
    PackageBundleRequest, StandardPackCatalogCutRequest, StandardPackCatalogCutResponse,
    StandardPackCatalogRequest, StandardPackCatalogResponse, StandardPackRootFiles,
    installed_root_containing, package_bundle,
};
use actingcommand_contract::codes::{ContractCode, SharedCode};
use actingcommand_contract::outcome::{
    Detail, IoOp, KeyKind, NAME_MAX_BYTES, Outcome, key_text_fits,
};
use actingcommand_contract::{
    BundleIndex, BundleMaintenance, CatalogDisplayFile, CatalogDisplayFileIssue, CatalogEdits,
    CatalogMergeRefusal, CatalogScheduleDifference, CatalogScheduleFacts, CatalogValuesFile,
    DeclaredCapabilities, IrreversiblePackages, LabError, LabErrorClass, LabResult,
    SCHEDULE_RULE_BUSINESS_MEMBERS, STANDARD_PACK_BUNDLE_FILE, STANDARD_PACK_CATALOG_FILE,
    STANDARD_PACK_CATALOG_SCHEMA_VERSION_V2, STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    STANDARD_PACK_INTERRUPTS_FILE, STANDARD_PACK_NAVIGATION_FILE, STANDARD_PACK_PAGES_FILE,
    STANDARD_PACK_RETIRED_UI_FILE, STANDARD_PACK_SCHEDULE_FILE, StandardPackProblem,
    StandardPackRegistry, StandardPackRoot, StandardPackRootBytes, StandardPackRootIssue,
    catalog_display_outcome, merge_standard_pack_catalog, parse_standard_pack_catalog_with,
    parse_standard_pack_schedule, read_standard_pack_root, retired_ui_outcome,
    standard_pack_catalog_cut_differences,
};
use actingcommand_policy::{
    CATALOG_BUSINESS_SUBSYSTEM, StandardPackComposeFailure, standard_pack_schedule_takes_catalog,
};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The answer's `status` of a plan.
const PLANNED: &str = "planned";
/// The answer's `status` of a line that wrote the file.
const WRITTEN: &str = "written";
/// The answer's `status` of a line that found the file holding the bytes already.
const UNCHANGED: &str = "unchanged";
/// A checkout's applications table, from which the build makes `bundle.json`.
const APPLICATIONS_FILE: &str = "applications.json";
/// A checkout's directory of package directories.
const PACKS_DIRECTORY: &str = "packs";
/// The scratch directories' names below the system's temporary folder,
/// `<prefix>-<process>-<count>`.
const SCRATCH_PREFIX: &str = "actinglab-pack-catalog";
/// The count of the scratch directories this process made, so that each call takes its own.
static SCRATCH_COUNT: AtomicU64 = AtomicU64::new(0);

/// `standard-pack catalog` (module doc).
pub fn write_standard_pack_catalog(
    request: StandardPackCatalogRequest,
) -> LabResult<StandardPackCatalogResponse> {
    refresh_ids_fit(&request.refresh)?;
    let root = writable_root(&request.root)?;
    let (game, registry, _) = root_registry(&root)?;
    let schedule_path = root.join(STANDARD_PACK_SCHEDULE_FILE);
    let schedule = read_bounded(&schedule_path)?
        .map(|bytes| parse_standard_pack_schedule(&bytes))
        .transpose()
        .map_err(|issue| root_invalid(&game, &schedule_path, &issue))?;
    // Workflow #477 CV-2: the rules of a root cut over write no business part, so none is brought
    // back from them: the catalog holds the only copy.
    let cut = schedule.as_ref().is_some_and(|schedule| {
        schedule
            .requires
            .iter()
            .any(|name| name == CATALOG_BUSINESS_SUBSYSTEM)
    });
    if cut && !request.refresh.is_empty() {
        return Err(switched_refused(
            &schedule_path,
            "--refresh would bring entries back to them",
        ));
    }
    let path = root.join(STANDARD_PACK_CATALOG_FILE);
    let existing = read_bounded(&path)?;
    // CV-2 fix round 1 (the coordinator's ruling): nor is a missing file made, or one of version 1
    // converted or brought up to date, from them; a new entry of the file of version 2 gets its
    // display only (`CatalogEdits::switched`).
    if cut {
        match existing.as_deref() {
            None => {
                return Err(switched_refused(
                    &schedule_path,
                    &format!("{} is missing and would be made of them", path.display()),
                ));
            }
            Some(bytes) if !version_2(bytes) => {
                return Err(switched_refused(
                    &schedule_path,
                    &format!(
                        "{} is not of version 2 and would be written of them",
                        path.display()
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    let retired_path = root.join(STANDARD_PACK_RETIRED_UI_FILE);
    let retired = read_bounded(&retired_path)?;
    // A leftover `ui.json` that is no JSON gives no display; its warning names it either way.
    let retired_document = retired
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok());
    // PC-2: the display file, read whole; the contract crate checks it.
    let displays = request
        .displays
        .as_deref()
        .map(|path| read_input_file(DISPLAYS_FLAG, path))
        .transpose()?;
    // Workflow #477 CV-1: the values file, read whole as the display file is.
    let values = request
        .values
        .as_deref()
        .map(|path| read_input_file(VALUES_FLAG, path))
        .transpose()?;
    let edits = CatalogEdits {
        refresh: &request.refresh,
        displays: displays.as_ref().map(|(path, bytes)| CatalogDisplayFile {
            path: path.as_path(),
            bytes: bytes.as_slice(),
        }),
        version_2: request.version_2,
        values: values.as_ref().map(|(path, bytes)| CatalogValuesFile {
            path: path.as_path(),
            bytes: bytes.as_slice(),
        }),
        switched: cut,
    };
    let merge = merge_standard_pack_catalog(
        &game,
        &registry,
        schedule.as_ref(),
        existing.as_deref(),
        retired_document.as_ref(),
        &edits,
    )
    .map_err(|refusal| match refusal {
        CatalogMergeRefusal::Refresh(refusal) => refused(
            refusal.outcome(&game),
            LabErrorClass::UsageValidation,
            format!(
                "--refresh {}: {} (standard-pack catalog brings up to date only an entry of a \
                 package the root's registry holds, each named once)",
                refusal.package_id, refusal.problem
            ),
        ),
        // Fix round 1: the first refusal names the file it is in (`schedule.json` for a rule's
        // pool that file does not declare).
        CatalogMergeRefusal::Root(issues) => catalog_refused(&game, &root, issues),
        CatalogMergeRefusal::Display { path, issues } => display_refused(
            &game,
            &path,
            issues,
            "the display file is checked by the catalog's own display checks",
        ),
        // Workflow #477 CV-1: the values file's refusal is the display file's, naming its file.
        CatalogMergeRefusal::Values { path, issues } => display_refused(
            &game,
            &path,
            issues,
            "the values file is checked by the catalog's own value and mark checks",
        ),
        CatalogMergeRefusal::ValuesNeedVersion2 => LabError::usage(format!(
            "{VALUES_FLAG} writes a fixed value and marks, which only a catalog of version 2 \
             carries: {} is of version 1 or absent; give {VERSION_FLAG} 2 to write version 2",
            path.display()
        )),
    })?;
    let changed = existing.as_deref() != Some(merge.bytes.as_slice());
    let mut warnings = merge
        .catalog
        .display_issues()
        .iter()
        .map(|issue| catalog_display_outcome(&game, &path, issue))
        .collect::<Vec<_>>();
    // Fix round 1: what the writer left out, warned of as well as answered.
    warnings.extend(
        merge
            .left_out
            .iter()
            .map(|left_out| left_out.outcome(&game, &path)),
    );
    // CV-2: a root cut over has no second copy of the business part to differ from.
    let differences = if cut { Vec::new() } else { merge.differences };
    warnings.extend(
        differences
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
        differences,
        display_from_ui: merge.display_from_ui,
        refreshed: (!request.refresh.is_empty()).then_some(merge.refreshed),
        display_written: request.displays.is_some().then_some(merge.display_written),
        groups_added: request.displays.is_some().then_some(merge.groups_added),
        converted: request.version_2.then_some(merge.converted),
        values_written: request.values.is_some().then_some(merge.values_written),
        document,
        warnings,
    })
}

/// The absolute directory `root` a line of `standard-pack catalog` writes in: a directory (a usage
/// error otherwise) outside every installation (`standard_pack_catalog_root_refused` otherwise,
/// before anything is read: an installed root takes the file through the build and `place`).
fn writable_root(root: &Path) -> LabResult<PathBuf> {
    let root = std::path::absolute(root)
        .map_err(|error| LabError::usage(format!("{}: {error}", root.display())))?;
    if !fs::metadata(&root).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(LabError::usage(format!(
            "--root {} is no directory",
            root.display()
        )));
    }
    if let Some(installation) = installed_root_containing(&root)? {
        return Err(refused(
            Outcome::new(SharedCode::StandardPackCatalogRootRefused)
                .path(&root)
                .install_root(&installation),
            LabErrorClass::UsageValidation,
            format!(
                "{} lies inside the installation {}; standard-pack catalog writes a resource \
                 repository's checkout or a built game root outside an installation only, and an \
                 installed root takes the file through standard-pack build and place",
                root.display(),
                installation.display()
            ),
        ));
    }
    Ok(root)
}

/// Workflow #477 CV-2 fix round 1 (register §1.508; the coordinator's ruling): the refusal of a
/// line that would take a business part from the rules of a switched root, whose `schedule.json`
/// (`schedule_path`) names `catalog_business` and whose rules write none (`what`: what the line
/// would do with them): `validation_failed`, nothing written. The root's business part lives in
/// its catalog alone, which is restored (from version control, a build's zip or a backup), never
/// rebuilt from the rules.
fn switched_refused(schedule_path: &Path, what: &str) -> LabError {
    LabError::usage(format!(
        "the root is switched: {} names {CATALOG_BUSINESS_SUBSYSTEM}, so its rules write no \
         business part and {what}; its business part lives in {STANDARD_PACK_CATALOG_FILE} alone \
         and must be restored, not rebuilt",
        schedule_path.display()
    ))
}

/// CV-2 fix round 1: whether the catalog `bytes` declares version 2.
fn version_2(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok_and(|document| {
        document.get("schema_version").and_then(Value::as_str)
            == Some(STANDARD_PACK_CATALOG_SCHEMA_VERSION_V2)
    })
}

/// The answer's `status` of a cut a root has had already (its `schedule.json` names
/// `catalog_business`).
const CUT_ALREADY: &str = UNCHANGED;

/// Workflow #477 CV-2 (register §1.508; `contracts/pack-catalog.md`, "Who writes it"; the
/// coordinator's ruling 7): `standard-pack catalog --cut` of the root `request.root` (a checkout
/// or a built game root, as a plain line takes, never one inside an installation). A root whose
/// `schedule.json` names `catalog_business` already is answered `unchanged`, done, and nothing is
/// read further. Otherwise the root is composed as it stands (its registry, the root files beside
/// `schedule.json`, `bundle.json`'s maintenance entries; its packages' declarations, which no
/// composed task reads since Workflow #455 G-R2x, taken as read and providing nothing), the
/// switched `schedule.json` is made in memory ([`switched_schedule`]) and composed with the root's
/// `pack-catalog.json` by the same composer, and a refusal of either composition is the line's
/// (`standard_pack_root_invalid` at the file it names: a catalog that is absent or of version 1
/// included). The two compositions must be byte for byte the same (their rules digest and catalog
/// hash, and who wrote each fixed value and valuation, which no digest holds) and no rule of the
/// file, a held one included, may differ from the part that would stand for it
/// ([`standard_pack_catalog_cut_differences`]); else nothing is written and the line is refused
/// `validation_failed` with each difference ([`cut_differs`]). With `--plan` nothing is written and
/// the switched document is answered (`planned`); without it the file is written atomically
/// (`written`; a failure is `standard_pack_catalog_write_failed` naming `schedule.json`).
pub fn cut_standard_pack_catalog(
    request: StandardPackCatalogCutRequest,
) -> LabResult<StandardPackCatalogCutResponse> {
    let root = writable_root(&request.root)?;
    let (game, registry, bundle) = root_registry(&root)?;
    let path = root.join(STANDARD_PACK_SCHEDULE_FILE);
    let Some(bytes) = read_bounded(&path)? else {
        return Err(LabError::usage(format!(
            "{} holds no {STANDARD_PACK_SCHEDULE_FILE}: it has no rules to cut over",
            root.display()
        )));
    };
    let schedule =
        parse_standard_pack_schedule(&bytes).map_err(|issue| root_invalid(&game, &path, &issue))?;
    let answer = |status: &str, rules_digest: Option<String>, document: Option<Value>| {
        StandardPackCatalogCutResponse {
            status: status.to_owned(),
            root: root.display().to_string(),
            game: game.clone(),
            path: path.display().to_string(),
            rules_digest,
            rules: if status == CUT_ALREADY {
                Vec::new()
            } else {
                schedule.rules.iter().map(|rule| rule.id.clone()).collect()
            },
            document,
        }
    };
    if standard_pack_schedule_takes_catalog(&bytes) {
        return Ok(answer(CUT_ALREADY, None, None));
    }
    let catalog = read_bounded(&root.join(STANDARD_PACK_CATALOG_FILE))?;
    let (checked, maintenance) = cut_root(&root, &game, &bundle)?;
    let irreversible = IrreversiblePackages::new(
        checked
            .registry
            .package_ids()
            .map(|package_id| {
                // Workflow #500 SV-2 (register §1.511): the cut reads no package, so it holds no
                // rule's values to a declaration; both compositions it compares read the same.
                let declared = DeclaredCapabilities {
                    provides: BTreeMap::new(),
                    remedies: BTreeSet::new(),
                    variables: None,
                };
                (package_id.to_owned(), Some(declared))
            })
            .collect(),
    );
    let compose = |schedule: &[u8]| {
        compose_per_server(
            &game,
            &checked,
            &maintenance,
            schedule,
            &irreversible,
            None,
            catalog.as_deref(),
        )
        .map_err(|failure| cut_refused(&failure, &game, &root))
    };
    let before = compose(&bytes)?;
    let switched = switched_schedule(&bytes, &path)?;
    let after = compose(&switched)?;
    // The second composition accepted the file, so its reader reads it.
    let facts = CatalogScheduleFacts::of(Some(&schedule));
    let read = parse_standard_pack_catalog_with(
        catalog.as_deref().unwrap_or_default(),
        &game,
        &registry,
        Some(&facts),
    )
    .map_err(|issues| catalog_refused(&game, &root, issues))?;
    let differences = standard_pack_catalog_cut_differences(&read, &schedule);
    let same = before.rules_digest() == after.rules_digest()
        && before.compiled().catalog_hash() == after.compiled().catalog_hash()
        && before.benefit_provenance() == after.benefit_provenance();
    if !same || !differences.is_empty() {
        return Err(cut_differs(&game, &path, &differences));
    }
    let digest = Some(after.rules_digest().to_owned());
    if request.plan {
        let document = serde_json::from_slice::<Value>(&switched).map_err(|error| {
            LabError::usage(format!(
                "the schedule.json standard-pack catalog --cut makes does not read back: {error}"
            ))
        })?;
        return Ok(answer(PLANNED, digest, Some(document)));
    }
    write_atomically(&path, &switched)?;
    Ok(answer(WRITTEN, digest, None))
}

/// CV-2: the game root `standard-pack catalog --cut` composes, `root` of `game` whose registry's
/// `bundle.json` is `bundle` (a checkout's the build's `package_bundle` makes): its root files
/// beside `schedule.json` read by the one structure check of every reader, and the bundle's
/// maintenance entries (a checkout's bundle names none: its `maintenance.json` is the build's, and
/// both compositions are made of the one root).
fn cut_root(
    root: &Path,
    game: &str,
    bundle: &[u8],
) -> LabResult<(StandardPackRoot, Vec<BundleMaintenance>)> {
    let mut files = BTreeMap::new();
    for name in [
        STANDARD_PACK_PAGES_FILE,
        STANDARD_PACK_INTERRUPTS_FILE,
        STANDARD_PACK_NAVIGATION_FILE,
        STANDARD_PACK_DISPLAY_LAYOUT_FILE,
    ] {
        if let Some(bytes) = read_bounded(&root.join(name))? {
            files.insert(name, bytes);
        }
    }
    let bytes = |name: &str| files.get(name).map(Vec::as_slice);
    let checked = read_standard_pack_root(
        StandardPackRootBytes {
            bundle: Some(bundle),
            pages: bytes(STANDARD_PACK_PAGES_FILE),
            interrupts: bytes(STANDARD_PACK_INTERRUPTS_FILE),
            navigation: bytes(STANDARD_PACK_NAVIGATION_FILE),
            tables: false,
            display_layout: bytes(STANDARD_PACK_DISPLAY_LAYOUT_FILE),
        },
        game,
    )
    .map_err(|issue| root_invalid(game, &root.join(issue.file), &issue))?
    .ok_or_else(|| LabError::usage(format!("{} holds no game root to cut over", root.display())))?;
    let maintenance = serde_json::from_slice::<BundleIndex>(bundle)
        .map(|index| index.maintenance().to_vec())
        .unwrap_or_default();
    Ok((checked, maintenance))
}

/// CV-2: the `schedule.json` of `bytes` (at `path`, which its reader accepted) cut over: every
/// member as written, in its order, but `requires`, which names `catalog_business` last (after
/// `schema_version` when the file has none), and each rule without its business members
/// ([`SCHEDULE_RULE_BUSINESS_MEMBERS`]); two-space indented with a line feed at the end, each line
/// ending as the file's do (a carriage return before each line feed when the file has one).
fn switched_schedule(bytes: &[u8], path: &Path) -> LabResult<Vec<u8>> {
    let unread = |error: String| {
        LabError::usage(format!(
            "{}: the file standard-pack catalog --cut switches does not read: {error}",
            path.display()
        ))
    };
    let Value::Object(document) =
        serde_json::from_slice::<Value>(bytes).map_err(|error| unread(error.to_string()))?
    else {
        return Err(unread("no JSON object".to_owned()));
    };
    let named = document.contains_key("requires");
    let mut switched = Map::new();
    for (name, value) in document {
        let value = match (name.as_str(), value) {
            ("requires", Value::Array(mut names)) => {
                names.push(Value::String(CATALOG_BUSINESS_SUBSYSTEM.to_owned()));
                Value::Array(names)
            }
            ("rules", Value::Array(rules)) => {
                Value::Array(rules.into_iter().map(without_business).collect())
            }
            (_, value) => value,
        };
        let opens = name == "schema_version";
        switched.insert(name, value);
        if opens && !named {
            switched.insert("requires".to_owned(), json!([CATALOG_BUSINESS_SUBSYSTEM]));
        }
    }
    let mut written = serde_json::to_vec_pretty(&Value::Object(switched))
        .map_err(|error| unread(error.to_string()))?;
    written.push(b'\n');
    if !bytes.windows(2).any(|pair| pair == b"\r\n") {
        return Ok(written);
    }
    let mut crlf = Vec::with_capacity(written.len() * 2);
    for byte in written {
        if byte == b'\n' {
            crlf.push(b'\r');
        }
        crlf.push(byte);
    }
    Ok(crlf)
}

/// CV-2: the rule `rule` without its business members, every other member as written, in its
/// order.
fn without_business(rule: Value) -> Value {
    match rule {
        Value::Object(members) => Value::Object(
            members
                .into_iter()
                .filter(|(name, _)| !SCHEDULE_RULE_BUSINESS_MEMBERS.contains(&name.as_str()))
                .collect(),
        ),
        other => other,
    }
}

/// CV-2: the refusal of a composition the cut made of the root `root` of `game`:
/// `standard_pack_root_invalid` for the first refusal of `failure`, at the file it names
/// (`schedule.json`, or `pack-catalog.json` for a part taken from there); the developer message
/// lists every refusal.
fn cut_refused(failure: &StandardPackComposeFailure, game: &str, root: &Path) -> LabError {
    let Some(first) = failure.issues().first() else {
        return LabError::usage(format!("{failure} (no issue named)"));
    };
    let mut error = root_invalid(game, &root.join(first.issue.file), &first.issue);
    error.message = format!("{}; {failure}", error.message);
    error
}

/// CV-2: the refusal of a cut whose two compositions are not the same, or where a rule differs
/// from the part that would stand for it: `validation_failed` (the line cannot be carried out as
/// the root stands: its catalog does not hold what its rules write), nothing written, with every
/// difference, rule by rule, in `details.differences` (`{package_id, pool_id?, side?, difference,
/// rule_id}`, the words of `catalog_difference`) and as one `standard_pack_catalog_schedule_differs`
/// each in `details.warnings`.
fn cut_differs(game: &str, path: &Path, differences: &[CatalogScheduleDifference]) -> LabError {
    let rules = differences
        .iter()
        .filter_map(|difference| difference.rule_id.as_deref())
        .collect::<BTreeSet<_>>();
    LabError::usage(format!(
        "{} is not cut over: with its rules' business part taken from {STANDARD_PACK_CATALOG_FILE} \
         the root would compose otherwise ({} difference(s) in {} rule(s)); bring the catalog's \
         entries up to the rules (standard-pack catalog --refresh) and cut again",
        path.display(),
        differences.len(),
        rules.len()
    ))
    .with_details(json!({
        "differences": differences,
        "warnings": differences
            .iter()
            .map(|difference| difference.outcome(game).envelope(Detail::Full))
            .collect::<Vec<_>>(),
    }))
}

/// Workflow #477 PC-2 fix round 1 (the independent review): each `--refresh` id is one an answer's
/// `package_id` (a name of the key table: at most [`NAME_MAX_BYTES`] bytes, no control character)
/// can carry; the first that is not is a usage error (`validation_failed`) before the root is read,
/// naming the parameter, the id's place and its length, never its value.
fn refresh_ids_fit(refresh: &[String]) -> LabResult<()> {
    match refresh
        .iter()
        .enumerate()
        .find(|(_, package_id)| !key_text_fits(KeyKind::Name, package_id))
    {
        Some((index, package_id)) => Err(LabError::usage(format!(
            "--refresh value {} ({} bytes) is no package id: a package id is at most \
             {NAME_MAX_BYTES} bytes, without a control character",
            index + 1,
            package_id.len()
        ))),
        None => Ok(()),
    }
}

/// Workflow #477 PC-2: the flag of the display file. Workflow #477 CV-1: the flags of the values
/// file and of the version asked for, which the refusals name.
const DISPLAYS_FLAG: &str = "--displays";
const VALUES_FLAG: &str = "--values";
const VERSION_FLAG: &str = "--schema-version";

/// Workflow #477 PC-2: the display file `--displays` names, an absolute path, and its bytes; CV-1:
/// and so the values file `--values` names (`flag` the one that names it). A relative path, a file
/// that does not exist, cannot be read or exceeds the package input byte limit is a usage error
/// naming it (`validation_failed`), as a root file's is; its content is the contract crate's to
/// check.
fn read_input_file(flag: &str, path: &Path) -> LabResult<(PathBuf, Vec<u8>)> {
    if !path.is_absolute() {
        return Err(LabError::usage(format!(
            "{flag} {} is no absolute path",
            path.display()
        )));
    }
    match read_bounded(path)? {
        Some(bytes) => Ok((path.to_owned(), bytes)),
        None => Err(LabError::usage(format!(
            "{flag} {}: no such file",
            path.display()
        ))),
    }
}

/// Workflow #477 PC-2: the refusal `standard_pack_catalog_display_refused` of the display file at
/// `path` of the game root of `game`, with its first error (the contract crate answers at least
/// one), of the class usage or validation: the line names a file that cannot be written into the
/// catalog as it stands. Workflow #477 CV-1: the same refusal of the values file (`checks` what
/// the message says the file is checked by).
fn display_refused(
    game: &str,
    path: &Path,
    issues: Vec<CatalogDisplayFileIssue>,
    checks: &str,
) -> LabError {
    let first = issues.into_iter().next().unwrap_or_else(whole_display_file);
    refused(
        first.outcome(game, path),
        LabErrorClass::UsageValidation,
        format!(
            "{}: standard_pack_problem={} json_pointer={} ({checks} before anything is written)",
            path.display(),
            first.problem,
            first.pointer
        ),
    )
}

/// The display file as a whole, `malformed`: a refusal without an error is the whole file's.
fn whole_display_file() -> CatalogDisplayFileIssue {
    CatalogDisplayFileIssue {
        problem: StandardPackProblem::Malformed,
        pointer: String::new(),
    }
}

/// The game and the registry of the directory `root` (module doc). A directory that holds
/// `bundle.json` is a built game root: its game is its directory's name and its registry that
/// file's. One that holds `applications.json` instead is a checkout ([`checkout_registry`]). One
/// that holds neither is `standard_pack_root_invalid`, `bundle_missing`, at `bundle.json`.
/// Workflow #477 CV-2: and the bytes of the `bundle.json` the registry is read from, which the cut
/// composes the root with.
fn root_registry(root: &Path) -> LabResult<(String, StandardPackRegistry, Vec<u8>)> {
    let bundle_path = root.join(STANDARD_PACK_BUNDLE_FILE);
    let named = || {
        root.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                LabError::usage(format!("--root {} names no game directory", root.display()))
            })
    };
    if let Some(bundle) = read_bounded(&bundle_path)? {
        let game = named()?;
        let registry = StandardPackRegistry::from_bundle_bytes(&bundle, &game)
            .map_err(|issue| root_invalid(&game, &bundle_path, &issue))?;
        return Ok((game, registry, bundle));
    }
    if fs::symlink_metadata(root.join(APPLICATIONS_FILE)).is_ok() {
        return checkout_registry(root);
    }
    let mut error = root_invalid(
        &named()?,
        &bundle_path,
        &StandardPackRootIssue {
            file: STANDARD_PACK_BUNDLE_FILE,
            problem: StandardPackProblem::BundleMissing,
            pointer: String::new(),
        },
    );
    error.message = format!(
        "{}; --root takes a resource repository's checkout (applications.json and packs/) or a \
         built game root (bundle.json)",
        error.message
    );
    Err(error)
}

/// The game and the registry of the checkout `root`: the build's own `package_bundle` of its
/// `applications.json` and of each directory of its `packs/` (a file there, such as a
/// `README.md`, is no package and is passed over, as the build's layout passes it over), with no
/// root file, made in a scratch directory of the system's temporary folder; the game is the
/// bundle's, which `applications.json` names, and the registry that of the `bundle.json` it writes.
/// The scratch directory is removed whatever the bundle's end. A refusal of `package_bundle` is
/// its own (a checkout without a package is `package_bundle_packs_empty`); an OS failure of the
/// scratch directory is `foreign_os_error` ([`scratch_failed`]), and one that cannot be removed
/// refuses the line too, naming it. Workflow #477 CV-2: and that `bundle.json`'s bytes.
fn checkout_registry(root: &Path) -> LabResult<(String, StandardPackRegistry, Vec<u8>)> {
    let scratch = make_scratch()?;
    let made = bundle_in(root, &scratch);
    let removed = fs::remove_dir_all(&scratch);
    match (made, removed) {
        (Ok(made), Ok(())) => Ok(made),
        (Ok(_), Err(error)) => Err(scratch_failed(&scratch, &error, IoOp::Remove)),
        (Err(mut error), Ok(())) => {
            error.message = format!(
                "{}; the scratch directory {} the writer made was removed",
                error.message,
                scratch.display()
            );
            Err(error)
        }
        (Err(mut error), Err(removal)) => {
            error.message = format!(
                "{}; the scratch directory {} the writer made could not be removed: {removal}",
                error.message,
                scratch.display()
            );
            Err(error)
        }
    }
}

/// A new scratch directory `<temporary folder>/actinglab-pack-catalog-<process>-<count>`; a name
/// that is taken already (a leftover of an earlier process) is passed over for the next count.
fn make_scratch() -> LabResult<PathBuf> {
    let temporary = std::env::temp_dir();
    loop {
        let scratch = temporary.join(format!(
            "{SCRATCH_PREFIX}-{}-{}",
            std::process::id(),
            SCRATCH_COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&scratch) {
            Ok(()) => return Ok(scratch),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(scratch_failed(&scratch, &error, IoOp::CreateDir)),
        }
    }
}

/// [`checkout_registry`]'s work in `scratch`: the directories of `<root>/packs` copied into
/// `<scratch>/packs-root`, in name order, then `package_bundle` into `<scratch>/bundle`.
fn bundle_in(root: &Path, scratch: &Path) -> LabResult<(String, StandardPackRegistry, Vec<u8>)> {
    let stage = scratch.join("packs-root");
    fs::create_dir(&stage).map_err(|error| scratch_failed(&stage, &error, IoOp::CreateDir))?;
    let packs = root.join(PACKS_DIRECTORY);
    let mut names = Vec::new();
    match fs::read_dir(&packs) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| scratch_failed(&packs, &error, IoOp::ReadDir))?;
                let source = entry.path();
                let metadata = fs::symlink_metadata(&source)
                    .map_err(|error| scratch_failed(&source, &error, IoOp::Metadata))?;
                if metadata.is_dir() {
                    names.push(entry.file_name());
                }
            }
        }
        // No `packs/`: `package_bundle` refuses the empty table.
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(scratch_failed(&packs, &error, IoOp::ReadDir)),
    }
    names.sort();
    for name in names {
        copy_tree(&packs.join(&name), &stage.join(&name))?;
    }
    let out = scratch.join("bundle");
    let response = package_bundle(PackageBundleRequest {
        applications: root.join(APPLICATIONS_FILE),
        packs_root: stage,
        out: out.clone(),
        source: None,
        maintenance: None,
        root_files: StandardPackRootFiles::default(),
    })?;
    let game = response.index.game;
    let bundle_path = out.join(STANDARD_PACK_BUNDLE_FILE);
    let bundle =
        fs::read(&bundle_path).map_err(|error| scratch_failed(&bundle_path, &error, IoOp::Read))?;
    let registry = StandardPackRegistry::from_bundle_bytes(&bundle, &game)
        .map_err(|issue| root_invalid(&game, &bundle_path, &issue))?;
    Ok((game, registry, bundle))
}

/// Copies the directory tree `from` into the new directory `to`, file by file, following links as
/// the build's copy of a checkout's packs does.
fn copy_tree(from: &Path, to: &Path) -> LabResult<()> {
    fs::create_dir(to).map_err(|error| scratch_failed(to, &error, IoOp::CreateDir))?;
    let entries =
        fs::read_dir(from).map_err(|error| scratch_failed(from, &error, IoOp::ReadDir))?;
    for entry in entries {
        let entry = entry.map_err(|error| scratch_failed(from, &error, IoOp::ReadDir))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let metadata = fs::metadata(&source)
            .map_err(|error| scratch_failed(&source, &error, IoOp::Metadata))?;
        if metadata.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            let bytes =
                fs::read(&source).map_err(|error| scratch_failed(&source, &error, IoOp::Read))?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .and_then(|mut file| file.write_all(&bytes))
                .map_err(|error| scratch_failed(&target, &error, IoOp::Write))?;
        }
    }
    Ok(())
}

/// Fix round 1: `foreign_os_error` at `path`, the operating system's error of `op` (a checkout's
/// `packs/` that cannot be read, or the scratch directory its registry is made in that cannot be
/// made, written, read or removed), of the class device or instance, as the Lab's other local
/// write failures: the line was right, the machine could not carry it out.
fn scratch_failed(path: &Path, error: &io::Error, op: IoOp) -> LabError {
    LabError::new(
        LabErrorClass::DeviceInstance,
        ContractCode::ForeignOsError.as_str(),
        format!(
            "{}: {error} (while making the registry of the checkout's packages)",
            path.display()
        ),
        &[],
    )
    .with_details(
        json!({"outcome": Outcome::from_io_error(error, op).path(path).envelope(Detail::Full)}),
    )
}

/// Writes `bytes` to `path` atomically: to `<path>.part` (made anew, synced), then renamed over
/// `path`. A failure is `standard_pack_catalog_write_failed` naming `path`, the OS error beneath
/// it, of the class device or instance (fix round 1: no usage refusal); the file in place is then
/// as it was.
fn write_atomically(path: &Path, bytes: &[u8]) -> LabResult<()> {
    let part = part_path(path);
    let failed = |error: &io::Error, op: IoOp| {
        refused(
            Outcome::new(SharedCode::StandardPackCatalogWriteFailed)
                .path(path)
                .caused_by(Outcome::from_io_error(error, op).path(&part)),
            LabErrorClass::DeviceInstance,
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

/// The Lab error of the refusal `outcome`, of `class`.
fn refused(outcome: Outcome, class: LabErrorClass, message: String) -> LabError {
    LabError::new(class, outcome.code().as_str(), message, &[])
        .with_details(json!({"outcome": outcome.envelope(Detail::Full)}))
}
