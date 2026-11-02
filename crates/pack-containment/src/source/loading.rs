// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #407 LOAD-2 (register §1.258; design `rd-1031/check-speed/model-loading.md` §2, §7
//! LOAD-2): a source pack's loading copy (`resources/loading.json`, `resources/loading/`),
//! compiled with the pack.
//!
//! The copy is read by the contract's one check (`parse_pack_loading`), then held against its
//! pack: the pack's `coordinate_space`, the declaration gate of its targets
//! ([`super::declarations`]), no page or target id the pack declares itself, and every template
//! it reads present and not of one colour; a file below `resources/loading/` it does not read is
//! a stale copy. Its targets join the derived recognition pack, as declared, with their ids; its
//! pages join the derived page set's member `loading` (`{max_ms, pages}`), beside its pages and
//! never in them, each id under the game as the pack's own pages are. Nothing runs on them yet:
//! LOAD-3 runs them.

use super::{
    Bundle, CliError, CliOutcome, OperationParser, PACK_SCHEMA_0_7, ParseOutputs, array_field,
    color_check_to_pack, color_target, pack_schema_version, pack_target, page_id, region_to_pack,
    required_field, required_string, split_template_mask, template_resource_path, with_luma_floor,
    with_mask, with_scales, with_unique,
};
use actingcommand_contract::{
    LoadingTarget, LoadingTargetFamily, PACK_LOADING_FILE, PackLoading, StandardPackProblem,
    pack_loading_entry,
};
use actingcommand_recognition::Scene;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The task schema a copy's template members are read in (a mask needs `0.9`).
const LOADING_TASK_SCHEMA: &str = "0.9";

/// Why a pack's loading copy is refused.
pub(crate) enum LoadingRefusal {
    /// A rule of the copy against its pack (`standard_pack_root_invalid`): its problem, the file
    /// inside the pack it names and the JSON pointer in the copy (empty for a whole file).
    Problem {
        problem: StandardPackProblem,
        file: String,
        pointer: String,
    },
    /// The declaration gate refused a member of the copy (`resource_declaration_invalid`), or a
    /// target could not be derived.
    Declaration(CliError),
}

impl From<CliError> for LoadingRefusal {
    fn from(error: CliError) -> Self {
        Self::Declaration(error)
    }
}

fn refused(problem: StandardPackProblem, file: &str, pointer: String) -> LoadingRefusal {
    LoadingRefusal::Problem {
        problem,
        file: file.to_owned(),
        pointer,
    }
}

/// Why a template cannot tell a loading page: it is no PNG this build decodes, or every pixel is
/// of one colour (a black or white screen is no loading page); `None` when it can. The check of
/// `package loading` (LOAD-1) and of the pack loader (LOAD-2).
pub fn loading_template_problem(bytes: &[u8]) -> Option<String> {
    let scene = match Scene::from_png(bytes) {
        Ok(scene) => scene,
        Err(error) => return Some(format!("no PNG this build decodes: {error}")),
    };
    let (pixels, _) = scene.rgb8_pixels().as_chunks::<3>();
    let mut pixels = pixels.iter();
    let first = pixels.next();
    if first.is_none() || pixels.all(|pixel| Some(pixel) == first) {
        return Some("every pixel is of one colour".to_owned());
    }
    None
}

/// Each target of `copy` with its family and its index in that family's list.
fn indexed_targets(copy: &PackLoading) -> Vec<(LoadingTargetFamily, usize, &LoadingTarget)> {
    let mut indexed = Vec::with_capacity(copy.targets.len());
    for family in LoadingTargetFamily::ALL {
        let targets = copy
            .targets
            .iter()
            .filter(|target| target.family() == family);
        indexed.extend(
            targets
                .enumerate()
                .map(|(index, target)| (family, index, target)),
        );
    }
    indexed
}

/// Compiles the copy `copy` of the pack `parser` parsed, whose entries are `entries`, into its
/// derived `outputs` (the module header).
pub(crate) fn compile_pack_loading(
    parser: &OperationParser,
    copy: &PackLoading,
    outputs: &mut ParseOutputs,
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<(), LoadingRefusal> {
    let frame = [
        u64::from(copy.coordinate_space.width),
        u64::from(copy.coordinate_space.height),
    ];
    let declared = ["width", "height"].map(|side| parser.coordinate_space[side].as_u64());
    if declared != [Some(frame[0]), Some(frame[1])] {
        return Err(refused(
            StandardPackProblem::LoadingCoordinateMismatch,
            PACK_LOADING_FILE,
            "/coordinate_space".to_owned(),
        ));
    }
    super::declarations::validate_pack_loading_declarations(
        copy,
        frame,
        parser.defaults.get("match_metric").and_then(Value::as_str),
    )?;
    let page_ids = array_field(&outputs.pages, "pages")
        .iter()
        .filter_map(|page| page.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    for (index, page) in copy.pages.iter().enumerate() {
        if page_ids.contains(page_id(&parser.game, &page.id).as_str()) {
            return Err(refused(
                StandardPackProblem::LoadingPageCollision,
                PACK_LOADING_FILE,
                format!("/pages/{index}/id"),
            ));
        }
    }
    let target_ids = array_field(&outputs.pack, "targets")
        .iter()
        .filter_map(|target| target.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let indexed = indexed_targets(copy);
    let mut read = BTreeSet::new();
    for (family, index, target) in &indexed {
        if target_ids.contains(target.id()) {
            return Err(refused(
                StandardPackProblem::LoadingTargetCollision,
                PACK_LOADING_FILE,
                format!("/{}/{index}/id", family.member()),
            ));
        }
        let Some(template) = target.template() else {
            continue;
        };
        let file = format!("resources/{template}");
        if entries
            .get(&file)
            .is_none_or(|bytes| loading_template_problem(bytes).is_some())
        {
            return Err(refused(
                StandardPackProblem::LoadingTemplateInvalid,
                PACK_LOADING_FILE,
                format!("/{}/{index}/template", family.member()),
            ));
        }
        read.insert(file);
    }
    // A file of a copy the copy does not read: the copy is not the one `package loading` writes.
    if let Some(stray) = entries.keys().find(|path| {
        pack_loading_entry(path) && path.as_str() != PACK_LOADING_FILE && !read.contains(*path)
    }) {
        return Err(refused(
            StandardPackProblem::LoadingPackStale,
            stray,
            String::new(),
        ));
    }
    let targets = derived_targets(parser, copy, &indexed)?;
    let pack_targets = outputs
        .pack
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CliError::package_invalid("the derived recognition pack has no targets"))?;
    pack_targets.extend_from_slice(&targets);
    if pack_schema_version(&targets, &[]) == PACK_SCHEMA_0_7 {
        outputs.pack["schema_version"] = Value::String(PACK_SCHEMA_0_7.to_owned());
    }
    let pages = copy
        .pages
        .iter()
        .map(|page| {
            let mut object = Map::new();
            object.insert("id".to_owned(), json!(page_id(&parser.game, &page.id)));
            object.insert("required".to_owned(), json!(page.required));
            if !page.any_of.is_empty() {
                object.insert("any_of".to_owned(), json!(page.any_of));
            }
            object.insert("optional".to_owned(), json!([]));
            object.insert("forbidden".to_owned(), json!(page.forbidden));
            Value::Object(object)
        })
        .collect::<Vec<_>>();
    outputs.pages["loading"] = json!({"max_ms": copy.max_ms, "pages": pages});
    Ok(())
}

/// The recognition targets of the copy, in its order, each as a `task.json` declaration of its
/// family derives it, under its own id: a template's threshold is its own, else the copy's
/// `defaults.template_threshold`, else the pack's; a colour target's `max_distance` is its own,
/// else the copy's `defaults.color_max_distance` (else the pack's default applies).
fn derived_targets(
    parser: &OperationParser,
    copy: &PackLoading,
    indexed: &[(LoadingTargetFamily, usize, &LoadingTarget)],
) -> CliOutcome<Vec<Value>> {
    let defaults = copy.defaults.clone().unwrap_or_default();
    // The copy's templates are relative to the pack's `resources/`, the parser's root.
    let bundle = Bundle {
        task_id: "loading".to_owned(),
        dir: parser.root.clone(),
        data: json!({"schema_version": LOADING_TASK_SCHEMA, "defaults": defaults.clone()}),
    };
    let mut targets = Vec::with_capacity(indexed.len());
    for (family, _, target) in indexed {
        let declaration = Value::Object(target.declaration().clone());
        let id = required_string(&declaration, "id")?;
        let derived = if family.is_template() {
            let (source, mask) = split_template_mask(&bundle, declaration)?;
            let template = required_string(&source, "template")?;
            let threshold = match source
                .get("threshold")
                .or_else(|| defaults.get("template_threshold"))
            {
                Some(threshold) => threshold.clone(),
                None => required_field(&parser.defaults, "template_threshold")?.clone(),
            };
            if !threshold.is_number() {
                return Err(CliError::package_invalid(format!(
                    "{PACK_LOADING_FILE}: the threshold of {id} must be a number"
                )));
            }
            let derived = pack_target(
                &source,
                &id,
                &template_resource_path(&parser.root, &bundle.dir, &template)?,
                region_to_pack(required_field(&source, "region")?)?,
                threshold,
                color_check_to_pack(source.get("color_check"), &id)?,
                None,
            )?;
            with_luma_floor(
                &source,
                with_mask(mask, with_unique(&source, with_scales(&source, derived))),
            )
        } else {
            let mut derived = color_target(
                &id,
                region_to_pack(required_field(&declaration, "region")?)?,
                required_field(&declaration, "expected")?.clone(),
                None,
            );
            if let Some(distance) = declaration
                .get("max_distance")
                .or_else(|| defaults.get("color_max_distance"))
            {
                derived["max_distance"] = distance.clone();
            }
            if let Some(alternatives) = declaration.get("alternatives") {
                derived["alternatives"] = alternatives.clone();
            }
            derived
        };
        targets.push(derived);
    }
    Ok(targets)
}
