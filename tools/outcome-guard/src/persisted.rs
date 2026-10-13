// SPDX-License-Identifier: AGPL-3.0-only

//! G6 persisted prose (Workflow #378 model section 9; slice A3 turns it on): no serde type of
//! the contract crate that a stored record reaches holds Runtime text, that is, a string-typed
//! member named `message`, `detail`, `native_detail`, `reason`, `description`, `note`, `text`,
//! `summary`, `hint` or `error`. The persisted types are the serde types reachable, member by
//! member, from the stored roots: a member whose type names another serde type of the crate
//! reaches it. Test code is skipped. Text that is data rather than Runtime prose (foreign
//! evidence, recognised text) is allowed by a bridge entry with its reason.

use crate::source::{Check, Finding, has_cfg_test, is_string_type};
use proc_macro2::TokenTree;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use syn::{Attribute, Fields, GenericArgument, Item, PathArguments, Type};

/// The member names that carry prose.
pub const PROSE_MEMBERS: &[&str] = &[
    "message",
    "detail",
    "native_detail",
    "reason",
    "description",
    "note",
    "text",
    "summary",
    "hint",
    "error",
];

/// The stored records: a ledger event's payload, the task-diagnostic artifact's header and
/// rows, and the ledger's migration marker.
pub const PERSISTED_ROOTS: &[&str] = &[
    "EventPayload",
    "TaskDiagnosticHeader",
    "TaskDiagnosticRecord",
    "LedgerMigrationRecord",
];

/// One member of a serde type.
#[derive(Clone)]
pub struct Member {
    /// `Type.member` or `Enum::Variant.member`; a tuple member is named by its index.
    pub item: String,
    pub name: Option<String>,
    pub ty: Type,
    pub line: usize,
}

/// One serde type a file defines.
#[derive(Clone)]
pub struct SerdeType {
    pub path: String,
    pub name: String,
    pub members: Vec<Member>,
}

/// The serde types a file defines outside test code: each struct or enum that derives
/// `Serialize` or `Deserialize`, or that the file implements either trait for.
pub fn serde_types(path: &str, source: &str) -> Result<Vec<SerdeType>, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    let mut items = Vec::new();
    flatten(&file.items, &mut items);
    let mut implemented = BTreeSet::new();
    for item in &items {
        let Item::Impl(implementation) = item else {
            continue;
        };
        let Some((_, trait_path, _)) = &implementation.trait_ else {
            continue;
        };
        let serde_trait = trait_path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "Serialize" || segment.ident == "Deserialize");
        if !serde_trait {
            continue;
        }
        if let Type::Path(target) = implementation.self_ty.as_ref()
            && let Some(last) = target.path.segments.last()
        {
            implemented.insert(last.ident.to_string());
        }
    }
    let mut types = Vec::new();
    for item in items {
        match item {
            Item::Struct(definition) => {
                let name = definition.ident.to_string();
                if derives_serde(&definition.attrs) || implemented.contains(&name) {
                    let mut members = Vec::new();
                    push_members(&name, &definition.fields, &mut members);
                    types.push(SerdeType {
                        path: path.to_owned(),
                        name,
                        members,
                    });
                }
            }
            Item::Enum(definition) => {
                let name = definition.ident.to_string();
                if derives_serde(&definition.attrs) || implemented.contains(&name) {
                    let mut members = Vec::new();
                    for variant in &definition.variants {
                        let owner = format!("{name}::{}", variant.ident);
                        push_members(&owner, &variant.fields, &mut members);
                    }
                    types.push(SerdeType {
                        path: path.to_owned(),
                        name,
                        members,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(types)
}

/// G6 findings: every prose member of a type a stored root reaches, with its file.
pub fn persisted_prose(types: &[SerdeType]) -> Vec<(String, Finding)> {
    let mut by_name = BTreeMap::<&str, Vec<&SerdeType>>::new();
    for serde_type in types {
        by_name
            .entry(serde_type.name.as_str())
            .or_default()
            .push(serde_type);
    }
    let mut queue = VecDeque::new();
    for root in PERSISTED_ROOTS {
        if let Some(found) = by_name.get(root) {
            queue.extend(found.iter().copied());
        }
    }
    let mut seen = BTreeSet::new();
    let mut findings = Vec::new();
    while let Some(serde_type) = queue.pop_front() {
        if !seen.insert((serde_type.path.as_str(), serde_type.name.as_str())) {
            continue;
        }
        for member in &serde_type.members {
            let prose = member
                .name
                .as_deref()
                .is_some_and(|name| PROSE_MEMBERS.contains(&name));
            if prose && is_string_type(&member.ty) {
                findings.push((
                    serde_type.path.clone(),
                    Finding {
                        check: Check::G6,
                        line: member.line,
                        item: Some(member.item.clone()),
                        detail: format!("persisted member {} is text", member.item),
                    },
                ));
            }
            let mut names = BTreeSet::new();
            type_names(&member.ty, &mut names);
            for name in names {
                if let Some(reached) = by_name.get(name.as_str()) {
                    queue.extend(reached.iter().copied());
                }
            }
        }
    }
    findings
}

/// The items of a file and of its inline modules, without test code.
fn flatten<'a>(items: &'a [Item], flat: &mut Vec<&'a Item>) {
    for item in items {
        match item {
            Item::Mod(module) => {
                if !has_cfg_test(&module.attrs)
                    && let Some((_, inner)) = &module.content
                {
                    flatten(inner, flat);
                }
            }
            Item::Struct(definition) if has_cfg_test(&definition.attrs) => {}
            Item::Enum(definition) if has_cfg_test(&definition.attrs) => {}
            Item::Impl(implementation) if has_cfg_test(&implementation.attrs) => {}
            _ => flat.push(item),
        }
    }
}

/// Whether a `#[derive(..)]` names `Serialize` or `Deserialize`.
fn derives_serde(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("derive")
            && match &attr.meta {
                syn::Meta::List(list) => list.tokens.clone().into_iter().any(|token| {
                    matches!(&token, TokenTree::Ident(ident) if ident == "Serialize" || ident == "Deserialize")
                }),
                _ => false,
            }
    })
}

fn push_members(owner: &str, fields: &Fields, members: &mut Vec<Member>) {
    for (index, field) in fields.iter().enumerate() {
        let name = field.ident.as_ref().map(ToString::to_string);
        let label = name.clone().unwrap_or_else(|| index.to_string());
        let line = field
            .ident
            .as_ref()
            .map_or(0, |ident| ident.span().start().line);
        members.push(Member {
            item: format!("{owner}.{label}"),
            name,
            ty: field.ty.clone(),
            line,
        });
    }
}

/// Every path segment a type names, its generic arguments included.
fn type_names(ty: &Type, names: &mut BTreeSet<String>) {
    match ty {
        Type::Path(path) => {
            for segment in &path.path.segments {
                names.insert(segment.ident.to_string());
                if let PathArguments::AngleBracketed(arguments) = &segment.arguments {
                    for argument in &arguments.args {
                        if let GenericArgument::Type(inner) = argument {
                            type_names(inner, names);
                        }
                    }
                }
            }
        }
        Type::Reference(reference) => type_names(&reference.elem, names),
        Type::Tuple(tuple) => {
            for element in &tuple.elems {
                type_names(element, names);
            }
        }
        Type::Array(array) => type_names(&array.elem, names),
        Type::Slice(slice) => type_names(&slice.elem, names),
        Type::Paren(inner) => type_names(&inner.elem, names),
        Type::Group(inner) => type_names(&inner.elem, names),
        _ => {}
    }
}
