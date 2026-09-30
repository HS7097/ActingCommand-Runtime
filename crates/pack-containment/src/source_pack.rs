// SPDX-License-Identifier: AGPL-3.0-only

//! Source-neutral in-memory assembly of an admitted source snapshot.

use super::*;
use crate::git_source::source_error;
use actingcommand_contract::safe_source_path;
use std::path::PathBuf;
use std::time::Instant;

/// The source directory uses the existing control/resource layout. Only original
/// operation declarations and their explicit dependencies enter pure conversion.
pub(super) fn compile(
    mut entries: BTreeMap<String, Vec<u8>>,
    limits: ContainmentLimits,
    deadline: Instant,
) -> ContainmentResult<(MemoryPackage, Value)> {
    use source::{Bundle, ConversionFiles, OperationConverter, SourceFile, SourceRead};
    if Instant::now() >= deadline {
        return Err(source_error("source_tree_deadline"));
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
                return Err(source_error("source_operation_path_invalid"));
            }
            let data = serde_json::from_slice(bytes)
                .map_err(|_| source_error("source_operation_invalid"))?;
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
        .ok_or_else(|| source_error("source_entry_operation_missing"))?;
    let required = |key: &str| {
        entry
            .data
            .get(key)
            .cloned()
            .ok_or_else(|| source_error("source_conversion_metadata_missing"))
    };
    let locale = entry
        .data
        .get("locale")
        .and_then(Value::as_str)
        .ok_or_else(|| source_error("source_locale_missing"))?;
    let stem = format!("{}.{}", control.game, control.server);
    let projection_path = format!("resources/navigation/{stem}.projection.json");
    let files = ConversionFiles {
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
                                SourceRead::BoundedBytes(limit) if bytes.len() as u64 > limit => {
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
    let converter = OperationConverter {
        root: PathBuf::from("resources"),
        game: source::canonical_game(&control.game)
            .map_err(|_| source_error("source_game_invalid"))?,
        server: source::canonical_server(&control.server)
            .map_err(|_| source_error("source_server_invalid"))?,
        locale: source::canonical_locale(locale)
            .map_err(|_| source_error("source_locale_invalid"))?,
        coordinate_space: required("coordinate_space")?,
        defaults: required("defaults")?,
        resource_ids: source::resource_ids(&resources)
            .map_err(|_| source_error("source_resources_invalid"))?,
        existing_navigation: Some(
            serde_json::json!({"control_points": resources.get("control_points").cloned().unwrap_or_else(|| serde_json::json!([]))}),
        ),
        bundles,
        maa_task_overlays: Default::default(),
    };
    converter
        .validate_bundles(&files)
        .map_err(|error| declaration_error("resources/operations", error))?;
    let outputs = converter
        .build_all(&files)
        .map_err(|error| declaration_error("resources/operations", error))?;
    let operation = converter
        .canonical_task(&control.entry_task_id)
        .map_err(|error| ContainmentError::PackParse {
            path: format!("resources/operations/{}/task.json", control.entry_task_id),
            message: error.to_string(),
        })?;
    let operation_bytes = serde_json::to_vec(&operation)
        .map_err(|_| source_error("source_conversion_encode_failed"))?
        .len() as u64;
    if operation_bytes > limits.max_entry_bytes {
        return Err(source_error("source_compiled_entry_limit"));
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
            return Err(source_error("source_tree_deadline"));
        }
        if entries.contains_key(&path) {
            return Err(source_error("source_contains_derived_output"));
        }
        entries.insert(
            path,
            serde_json::to_vec(&value)
                .map_err(|_| source_error("source_conversion_encode_failed"))?,
        );
    }
    if entries.contains_key("resources/manifest.json") {
        return Err(source_error("source_contains_derived_output"));
    }
    // This in-memory index preserves the existing projection dependency guards;
    // the admitted package identity remains the verified source reference.
    let hashes: BTreeMap<_, _> = entries
        .iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix("resources/")
                .map(|path| (path.to_owned(), Sha256Hash::digest(bytes).to_string()))
        })
        .collect();
    entries.insert(
        "resources/manifest.json".to_owned(),
        serde_json::to_vec(
            &serde_json::json!({"entry_task_id":control.entry_task_id,"hashes":hashes}),
        )
        .map_err(|_| source_error("source_conversion_encode_failed"))?,
    );
    // Projection declarations remain the exact verified source bytes; build_all
    // has already checked them against the generated catalog.
    let mut resident_bytes = operation_bytes;
    for bytes in entries.values() {
        if bytes.len() as u64 > limits.max_entry_bytes {
            return Err(source_error("source_compiled_entry_limit"));
        }
        resident_bytes = resident_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| source_error("source_compiled_size_limit"))?;
    }
    if entries.len() > limits.max_entry_count
        || resident_bytes > limits.max_total_decompressed_bytes
        || resident_bytes > limits.max_resident_bytes_per_instance
    {
        return Err(source_error("source_compiled_size_limit"));
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
