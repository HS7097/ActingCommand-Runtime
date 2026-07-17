// SPDX-License-Identifier: AGPL-3.0-only

//! Pure declaration transformation for Runtime-controlled proposal previews and promotion.

use crate::policy_host::CatalogGeneration;
use crate::{RuntimeHostError, RuntimeHostResult};
use actingcommand_contract::{
    CatalogDeclarationPatch, CatalogProposal, ProposalDocument, ProposalKind,
    ProposalPatchOperation, ProposalPreview, RuntimeErrorCode, TaskTemplateInstantiation,
};
use actingcommand_policy::{
    CatalogDocumentSource, CatalogSources, CompiledCatalog, compile_catalog,
};
use serde_json::{Map, Value};

pub(crate) enum PreparedProposal {
    Ready {
        preview: ProposalPreview,
        sources: CatalogSources,
    },
    NeedsHumanSpecification {
        preview: ProposalPreview,
    },
}

impl PreparedProposal {
    pub(crate) const fn preview(&self) -> &ProposalPreview {
        match self {
            Self::Ready { preview, .. } | Self::NeedsHumanSpecification { preview } => preview,
        }
    }

    pub(crate) fn into_ready(self) -> RuntimeHostResult<(ProposalPreview, CatalogSources)> {
        match self {
            Self::Ready { preview, sources } => Ok((preview, sources)),
            Self::NeedsHumanSpecification { .. } => Err(request(
                "proposal_requires_human_specification",
                "promote_proposal",
            )),
        }
    }
}

pub(crate) fn prepare_proposal(
    active: &CatalogGeneration,
    sources: &CatalogSources,
    proposal: &CatalogProposal,
) -> RuntimeHostResult<PreparedProposal> {
    proposal
        .validate()
        .map_err(|_| request("proposal_invalid", "compile_proposal"))?;
    if proposal.base_catalog_hash() != active.catalog_hash()
        || proposal.base_catalog_version() != active.catalog_version()
    {
        return Err(request("proposal_base_catalog_changed", "compile_proposal"));
    }
    match proposal.proposal() {
        ProposalKind::LanguageExtension { .. } => Ok(PreparedProposal::NeedsHumanSpecification {
            preview: ProposalPreview::needs_human_specification(proposal)
                .map_err(|_| fatal("proposal_preview_invalid", "compile_proposal"))?,
        }),
        ProposalKind::ParameterInstantiation { instantiation } => compile_ready(
            proposal,
            instantiate_template(sources, proposal, instantiation)?,
        ),
        ProposalKind::CatalogDiff { patches } => {
            compile_ready(proposal, apply_catalog_diff(sources, patches)?)
        }
    }
}

fn compile_ready(
    proposal: &CatalogProposal,
    sources: CatalogSources,
) -> RuntimeHostResult<PreparedProposal> {
    let compiled = compile_catalog(&sources)
        .map_err(|_| request("proposal_catalog_compile_failed", "compile_proposal"))?;
    validate_compiled_target(proposal, &compiled)?;
    let preview = ProposalPreview::ready(proposal, compiled.catalog_hash().to_owned())
        .map_err(|_| fatal("proposal_preview_invalid", "compile_proposal"))?;
    Ok(PreparedProposal::Ready { preview, sources })
}

fn validate_compiled_target(
    proposal: &CatalogProposal,
    compiled: &CompiledCatalog,
) -> RuntimeHostResult<()> {
    if compiled.summary().catalog_version != proposal.target_catalog_version() {
        return Err(request(
            "proposal_target_version_mismatch",
            "compile_proposal",
        ));
    }
    Ok(())
}

fn instantiate_template(
    sources: &CatalogSources,
    proposal: &CatalogProposal,
    instantiation: &TaskTemplateInstantiation,
) -> RuntimeHostResult<CatalogSources> {
    let mut documents = ProposalDocuments::parse(sources)?;
    documents.set_catalog_version(proposal.target_catalog_version())?;
    let tasks = documents
        .tasks
        .get_mut("tasks")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| fatal("proposal_tasks_shape_invalid", "instantiate_template"))?;
    if tasks
        .iter()
        .any(|task| task.get("id").and_then(Value::as_str) == Some(instantiation.new_task_id()))
    {
        return Err(request(
            "proposal_task_identity_conflict",
            "instantiate_template",
        ));
    }
    let mut task = tasks
        .iter()
        .find(|task| {
            task.get("id").and_then(Value::as_str) == Some(instantiation.template_task_id())
        })
        .cloned()
        .ok_or_else(|| request("proposal_template_unknown", "instantiate_template"))?;
    let object = task
        .as_object_mut()
        .ok_or_else(|| fatal("proposal_template_shape_invalid", "instantiate_template"))?;
    object.insert(
        "id".to_owned(),
        Value::String(instantiation.new_task_id().to_owned()),
    );
    object.insert(
        "scope".to_owned(),
        serde_json::json!({
            "kind": "instance",
            "instance_id": instantiation.instance_id()
        }),
    );
    object.insert("instance_overrides".to_owned(), Value::Array(Vec::new()));
    if let Some(priority) = instantiation.priority() {
        object.insert("priority".to_owned(), Value::from(priority));
    }
    if let Some(weight) = instantiation.strategic_weight_milli() {
        object.insert("strategic_weight_milli".to_owned(), Value::from(weight));
    }
    if object
        .get("feedback_stop")
        .and_then(Value::as_object)
        .and_then(|feedback| feedback.get("task_id"))
        .and_then(Value::as_str)
        == Some(instantiation.template_task_id())
        && let Some(feedback) = object
            .get_mut("feedback_stop")
            .and_then(Value::as_object_mut)
    {
        feedback.insert(
            "task_id".to_owned(),
            Value::String(instantiation.new_task_id().to_owned()),
        );
    }
    tasks.push(task);
    documents.encode(sources)
}

fn apply_catalog_diff(
    sources: &CatalogSources,
    patches: &[CatalogDeclarationPatch],
) -> RuntimeHostResult<CatalogSources> {
    let mut documents = ProposalDocuments::parse(sources)?;
    for patch in patches {
        apply_patch(documents.document_mut(patch.document()), patch)?;
    }
    documents.encode(sources)
}

struct ProposalDocuments {
    tasks: Value,
    pools: Value,
    activity: Value,
    timeline: Value,
}

impl ProposalDocuments {
    fn parse(sources: &CatalogSources) -> RuntimeHostResult<Self> {
        Ok(Self {
            tasks: parse_document(&sources.tasks)?,
            pools: parse_document(&sources.pools)?,
            activity: parse_document(&sources.activity)?,
            timeline: parse_document(&sources.timeline)?,
        })
    }

    fn set_catalog_version(&mut self, version: u64) -> RuntimeHostResult<()> {
        for document in [
            &mut self.tasks,
            &mut self.pools,
            &mut self.activity,
            &mut self.timeline,
        ] {
            let catalog = document
                .get_mut("catalog")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| fatal("proposal_catalog_shape_invalid", "instantiate_template"))?;
            catalog.insert("catalog_version".to_owned(), Value::from(version));
        }
        Ok(())
    }

    fn document_mut(&mut self, document: ProposalDocument) -> &mut Value {
        match document {
            ProposalDocument::Tasks => &mut self.tasks,
            ProposalDocument::Pools => &mut self.pools,
            ProposalDocument::Activity => &mut self.activity,
            ProposalDocument::Timeline => &mut self.timeline,
        }
    }

    fn encode(self, sources: &CatalogSources) -> RuntimeHostResult<CatalogSources> {
        Ok(CatalogSources {
            tasks: encode_document(&sources.tasks, self.tasks)?,
            pools: encode_document(&sources.pools, self.pools)?,
            activity: encode_document(&sources.activity, self.activity)?,
            timeline: encode_document(&sources.timeline, self.timeline)?,
        })
    }
}

fn parse_document(source: &CatalogDocumentSource) -> RuntimeHostResult<Value> {
    serde_json::from_slice(&source.bytes)
        .map_err(|_| fatal("proposal_base_document_invalid", "compile_proposal"))
}

fn encode_document(
    source: &CatalogDocumentSource,
    document: Value,
) -> RuntimeHostResult<CatalogDocumentSource> {
    let bytes = serde_json::to_vec(&document)
        .map_err(|_| fatal("proposal_document_encode_failed", "compile_proposal"))?;
    Ok(CatalogDocumentSource::new(source.source_uri.clone(), bytes))
}

fn apply_patch(document: &mut Value, patch: &CatalogDeclarationPatch) -> RuntimeHostResult<()> {
    let mut tokens = decode_pointer(patch.path())?;
    let leaf = tokens
        .pop()
        .ok_or_else(|| request("proposal_patch_path_invalid", "apply_proposal_patch"))?;
    let parent = descend_mut(document, &tokens)?;
    let value = patch
        .value_json()
        .map(|value| {
            serde_json::from_str::<Value>(value)
                .map_err(|_| request("proposal_patch_value_invalid", "apply_proposal_patch"))
        })
        .transpose()?;
    match parent {
        Value::Object(object) => apply_object_patch(object, &leaf, patch.operation(), value),
        Value::Array(array) => apply_array_patch(array, &leaf, patch.operation(), value),
        _ => Err(request(
            "proposal_patch_parent_invalid",
            "apply_proposal_patch",
        )),
    }
}

fn apply_object_patch(
    object: &mut Map<String, Value>,
    key: &str,
    operation: ProposalPatchOperation,
    value: Option<Value>,
) -> RuntimeHostResult<()> {
    match operation {
        ProposalPatchOperation::Add if !object.contains_key(key) => {
            object.insert(key.to_owned(), required_patch_value(value)?);
            Ok(())
        }
        ProposalPatchOperation::Replace if object.contains_key(key) => {
            object.insert(key.to_owned(), required_patch_value(value)?);
            Ok(())
        }
        ProposalPatchOperation::Remove if object.remove(key).is_some() => Ok(()),
        _ => Err(request(
            "proposal_patch_target_conflict",
            "apply_proposal_patch",
        )),
    }
}

fn apply_array_patch(
    array: &mut Vec<Value>,
    token: &str,
    operation: ProposalPatchOperation,
    value: Option<Value>,
) -> RuntimeHostResult<()> {
    if operation == ProposalPatchOperation::Add && token == "-" {
        array.push(required_patch_value(value)?);
        return Ok(());
    }
    let index = parse_array_index(token)?;
    match operation {
        ProposalPatchOperation::Add if index <= array.len() => {
            array.insert(index, required_patch_value(value)?);
            Ok(())
        }
        ProposalPatchOperation::Replace if index < array.len() => {
            array[index] = required_patch_value(value)?;
            Ok(())
        }
        ProposalPatchOperation::Remove if index < array.len() => {
            array.remove(index);
            Ok(())
        }
        _ => Err(request(
            "proposal_patch_target_conflict",
            "apply_proposal_patch",
        )),
    }
}

fn required_patch_value(value: Option<Value>) -> RuntimeHostResult<Value> {
    value.ok_or_else(|| request("proposal_patch_value_missing", "apply_proposal_patch"))
}

fn descend_mut<'a>(document: &'a mut Value, tokens: &[String]) -> RuntimeHostResult<&'a mut Value> {
    let mut current = document;
    for token in tokens {
        current = match current {
            Value::Object(object) => object.get_mut(token),
            Value::Array(array) => array.get_mut(parse_array_index(token)?),
            _ => None,
        }
        .ok_or_else(|| request("proposal_patch_parent_missing", "apply_proposal_patch"))?;
    }
    Ok(current)
}

fn decode_pointer(path: &str) -> RuntimeHostResult<Vec<String>> {
    path.split('/')
        .skip(1)
        .map(|token| {
            if token.is_empty() {
                return Err(request(
                    "proposal_patch_path_invalid",
                    "apply_proposal_patch",
                ));
            }
            let mut decoded = String::with_capacity(token.len());
            let mut chars = token.chars();
            while let Some(character) = chars.next() {
                if character != '~' {
                    decoded.push(character);
                    continue;
                }
                decoded.push(match chars.next() {
                    Some('0') => '~',
                    Some('1') => '/',
                    _ => {
                        return Err(request(
                            "proposal_patch_path_invalid",
                            "apply_proposal_patch",
                        ));
                    }
                });
            }
            Ok(decoded)
        })
        .collect()
}

fn parse_array_index(token: &str) -> RuntimeHostResult<usize> {
    if token.is_empty() || (token.len() > 1 && token.starts_with('0')) {
        return Err(request(
            "proposal_patch_index_invalid",
            "apply_proposal_patch",
        ));
    }
    token
        .parse::<usize>()
        .map_err(|_| request("proposal_patch_index_invalid", "apply_proposal_patch"))
}

fn request(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::request(code, operation, RuntimeErrorCode::InvalidRequest)
}

fn fatal(code: &'static str, operation: &'static str) -> RuntimeHostError {
    RuntimeHostError::fatal(code, operation, RuntimeErrorCode::RuntimeFatal)
}
