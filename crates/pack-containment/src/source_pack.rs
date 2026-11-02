// SPDX-License-Identifier: AGPL-3.0-only

//! Source-neutral in-memory assembly of an admitted source snapshot.

use super::*;
use actingcommand_contract::codes::ContractCode;
use actingcommand_contract::outcome::RawSource;
use actingcommand_contract::safe_source_path;
use std::path::PathBuf;
use std::time::Instant;

/// The source directory uses the existing control/resource layout. Only original
/// operation declarations and their explicit dependencies enter pure parsing.
pub(super) fn compile(
    mut entries: BTreeMap<String, Vec<u8>>,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<(MemoryPackage, Value)> {
    use source::{Bundle, OperationParser, ParseFiles, SourceFile, SourceRead};
    if Instant::now() >= deadline {
        return Err(source_error(PackContainmentCode::SourceTreeDeadline));
    }
    let control: LabControl = read_json_entry(&entries, "control.json")?;
    let resources = read_json_value_entry(&entries, "resources/operations/resources.json")?;
    source::validate_resource_declarations(
        Path::new("resources/operations/resources.json"),
        &resources,
    )
    .map_err(|error| declaration_error("resources/operations/resources.json", error))?;
    let mut bundles = Vec::new();
    for (path, bytes) in &entries {
        if let Some(task) = path
            .strip_prefix("resources/operations/")
            .and_then(|path| path.strip_suffix("/task.json"))
        {
            if task.contains('/') || !safe_source_path(task) {
                return Err(source_error(
                    PackContainmentCode::SourceOperationPathInvalid,
                ));
            }
            let data =
                serde_json::from_slice(bytes).map_err(|error| operation_unparsed(path, &error))?;
            bundles.push(Bundle {
                task_id: task.to_owned(),
                dir: PathBuf::from(format!("resources/operations/{task}")),
                data,
            });
        }
    }
    source::declaration_file_requests(&bundles)
        .map_err(|error| declaration_error("resources/operations", error))?;
    let entry = bundles
        .iter()
        .find(|bundle| bundle.task_id == control.entry_task_id)
        .ok_or_else(|| source_error(PackContainmentCode::SourceEntryOperationMissing))?;
    let required = |key: &str| {
        entry
            .data
            .get(key)
            .cloned()
            .ok_or_else(|| source_error(PackContainmentCode::SourceConversionMetadataMissing))
    };
    let locale = entry
        .data
        .get("locale")
        .and_then(Value::as_str)
        .ok_or_else(|| source_error(PackContainmentCode::SourceLocaleMissing))?;
    let stem = format!("{}.{}", control.game, control.server);
    let projection_path = format!("resources/navigation/{stem}.projection.json");
    let files = ParseFiles {
        files: Arc::new(
            source::source_file_requests(&bundles)
                .into_iter()
                .map(|(path, read)| {
                    let bytes = path
                        .to_str()
                        .and_then(|path| entries.get(&path.replace('\\', "/")));
                    let file = match bytes {
                        Some(bytes) => SourceFile {
                            is_file: true,
                            length: Ok(bytes.len() as u64),
                            bytes: match read {
                                SourceRead::Metadata => {
                                    Err("metadata-only conversion input".to_owned())
                                }
                                SourceRead::BoundedBytes(limit)
                                | SourceRead::SelectionPolicy(limit)
                                    if bytes.len() as u64 > limit =>
                                {
                                    Err("conversion input exceeds declared limit".to_owned())
                                }
                                _ => Ok(bytes.clone()),
                            },
                        },
                        None => SourceFile {
                            is_file: false,
                            length: Err("source dependency missing".to_owned()),
                            bytes: Err("source dependency missing".to_owned()),
                        },
                    };
                    (path, file)
                })
                .collect(),
        ),
        projection_bytes: Ok(entries.get(&projection_path).cloned()),
        projection_exists: Ok(entries.contains_key(&projection_path)),
    };
    let parser = OperationParser {
        root: PathBuf::from("resources"),
        game: source::canonical_game(&control.game)
            .map_err(|_| source_error(PackContainmentCode::SourceGameInvalid))?,
        server: source::canonical_server(&control.server)
            .map_err(|_| source_error(PackContainmentCode::SourceServerInvalid))?,
        locale: source::canonical_locale(locale)
            .map_err(|_| source_error(PackContainmentCode::SourceLocaleInvalid))?,
        coordinate_space: required("coordinate_space")?,
        defaults: required("defaults")?,
        resource_ids: source::resource_ids(&resources)
            .map_err(|_| source_error(PackContainmentCode::SourceResourcesInvalid))?,
        existing_navigation: Some(
            serde_json::json!({"control_points": resources.get("control_points").cloned().unwrap_or_else(|| serde_json::json!([]))}),
        ),
        bundles,
        maa_task_overlays: Default::default(),
    };
    parser
        .validate_bundles(&files)
        .map_err(|error| declaration_error("resources/operations", error))?;
    let mut outputs = parser
        .build_all(&files)
        .map_err(|error| declaration_error("resources/operations", error))?;
    // Workflow #407 LOAD-2: the pack's loading copy joins its derived recognition outputs.
    compile_loading(&control, &parser, &mut outputs, &entries)?;
    let operation = parser
        .canonical_task(&control.entry_task_id)
        .map_err(|error| ContainmentError::PackParse {
            path: format!("resources/operations/{}/task.json", control.entry_task_id),
            message: error.to_string(),
        })?;
    let operation_bytes = serde_json::to_vec(&operation)
        .map_err(|_| source_error(PackContainmentCode::SourceConversionEncodeFailed))?
        .len() as u64;
    if operation_bytes > limits.max_entry_bytes {
        return Err(source_error(PackContainmentCode::SourceCompiledEntryLimit));
    }
    drop(files);
    let generated = [
        (
            format!("resources/recognition/{stem}.pack.json"),
            outputs.pack,
        ),
        (
            format!("resources/recognition/{stem}.pages.json"),
            outputs.pages,
        ),
        (
            format!("resources/navigation/{stem}.navigation.json"),
            outputs.navigation,
        ),
        (
            "resources/operations/operations.index.json".to_owned(),
            outputs.index,
        ),
        (
            "resources/operations/operations.primitives.json".to_owned(),
            outputs.primitives,
        ),
    ];
    for (path, value) in generated {
        if Instant::now() >= deadline {
            return Err(source_error(PackContainmentCode::SourceTreeDeadline));
        }
        if entries.contains_key(&path) {
            return Err(source_error(
                PackContainmentCode::SourceContainsDerivedOutput,
            ));
        }
        entries.insert(
            path,
            serde_json::to_vec(&value)
                .map_err(|_| source_error(PackContainmentCode::SourceConversionEncodeFailed))?,
        );
    }
    if entries.contains_key("resources/manifest.json") {
        return Err(source_error(
            PackContainmentCode::SourceContainsDerivedOutput,
        ));
    }
    // Assembly reads the derived manifest for its entry task; it carries no file hashes
    // (Workflow #336 PKG-S1: no pin inside a package is computed or compared).
    entries.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(&serde_json::json!({"entry_task_id":control.entry_task_id}))
            .map_err(|_| source_error(PackContainmentCode::SourceConversionEncodeFailed))?,
    );
    // Projection declarations remain the exact source bytes read; build_all has already
    // checked them against the generated catalog.
    let mut resident_bytes = operation_bytes;
    for bytes in entries.values() {
        if bytes.len() as u64 > limits.max_entry_bytes {
            return Err(source_error(PackContainmentCode::SourceCompiledEntryLimit));
        }
        resident_bytes = resident_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| source_error(PackContainmentCode::SourceCompiledSizeLimit))?;
    }
    if entries.len() > limits.max_entry_count
        || resident_bytes > limits.max_total_decompressed_bytes
        || resident_bytes > limits.max_resident_bytes_per_instance
    {
        return Err(source_error(PackContainmentCode::SourceCompiledSizeLimit));
    }
    Ok((
        MemoryPackage {
            entry_count: entries.len(),
            entries,
            resident_bytes,
        },
        operation,
    ))
}

/// Workflow #407 LOAD-2 (register §1.258): the pack's loading copy, when it carries one, read
/// and compiled into `outputs` (`source::compile_pack_loading`). A copy that breaks a rule is
/// `standard_pack_root_invalid` with its `game`, `path` (the file inside the pack), problem,
/// `json_pointer` and `package_id`, the first issue of a copy that does not read; a member the
/// declaration gate refuses is `resource_declaration_invalid` in the copy. A file below
/// `resources/loading/` without the copy is a stale copy. A pack without either compiles as before.
fn compile_loading(
    control: &LabControl,
    parser: &source::OperationParser,
    outputs: &mut source::ParseOutputs,
    entries: &BTreeMap<String, Vec<u8>>,
) -> ContainmentResult<()> {
    use actingcommand_contract::{PACK_LOADING_FILE, StandardPackProblem, parse_pack_loading};
    let refused = |problem: StandardPackProblem, file: &str, pointer: &str| {
        let code = SharedCode::StandardPackRootInvalid;
        let outcome = Outcome::new(code)
            .game(control.game.as_str())
            .path(file)
            .standard_pack_problem(problem)
            .json_pointer(pointer);
        ContainmentError::LocatedSourceTree {
            code: code.code(),
            outcome: match control.package_id.as_deref() {
                Some(package_id) => outcome.package_id(package_id),
                None => outcome,
            },
        }
    };
    let Some(bytes) = entries.get(PACK_LOADING_FILE) else {
        return match entries
            .keys()
            .find(|path| actingcommand_contract::pack_loading_entry(path))
        {
            Some(stray) => Err(refused(StandardPackProblem::LoadingPackStale, stray, "")),
            None => Ok(()),
        };
    };
    let copy = parse_pack_loading(bytes).map_err(|issues| match issues.first() {
        Some(issue) => refused(issue.problem, issue.file, &issue.pointer),
        None => refused(StandardPackProblem::Malformed, PACK_LOADING_FILE, ""),
    })?;
    source::compile_pack_loading(parser, &copy, outputs, entries).map_err(|refusal| match refusal {
        source::LoadingRefusal::Problem {
            problem,
            file,
            pointer,
        } => refused(problem, &file, &pointer),
        source::LoadingRefusal::Declaration(error) => declaration_error(PACK_LOADING_FILE, error),
    })
}

/// Workflow #411 A41: `source_operation_invalid` for a task declaration that is no JSON document,
/// located: the declaration's `path` inside the package and, where the parser gives one, its
/// position (`line` and `column`), caused by the parser's own text (`foreign_data_error`).
fn operation_unparsed(path: &str, error: &serde_json::Error) -> ContainmentError {
    let code = PackContainmentCode::SourceOperationInvalid;
    let outcome = Outcome::new(code).path(path);
    let outcome = if error.line() > 0 {
        outcome.line(error.line()).column(error.column())
    } else {
        outcome
    };
    let parser = Outcome::new(ContractCode::ForeignDataError)
        .raw_source(RawSource::Json)
        .raw_text(error.to_string());
    ContainmentError::LocatedSourceTree {
        code: code.code(),
        outcome: outcome.caused_by(parser),
    }
}
