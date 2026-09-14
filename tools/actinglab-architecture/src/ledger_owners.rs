// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Attribute, Item, Meta, Token, UseTree};

use super::{LocalTypeAliases, local_type_aliases, resolve_alias};

/// One production module scope, with its own parsed items and source identity.
pub struct LedgerOwnerModule {
    pub path: PathBuf,
    pub module: String,
    pub(crate) items: Vec<Item>,
    pub(crate) aliases: LocalTypeAliases,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Condition {
    True,
    False,
    Unknown,
}

fn nested(list: &syn::MetaList) -> Result<Vec<Meta>, String> {
    Punctuated::<Meta, Token![,]>::parse_terminated
        .parse2(list.tokens.clone())
        .map(|values| values.into_iter().collect())
        .map_err(|error| format!("unresolved module attribute: {error}"))
}

fn condition(meta: &Meta) -> Result<Condition, String> {
    match meta {
        Meta::Path(path) if path.is_ident("test") => Ok(Condition::False),
        Meta::Path(_) | Meta::NameValue(_) => Ok(Condition::Unknown),
        Meta::List(list) => {
            let values = nested(list)?
                .iter()
                .map(condition)
                .collect::<Result<Vec<_>, _>>()?;
            if list.path.is_ident("not") && values.len() == 1 {
                return Ok(match values[0] {
                    Condition::True => Condition::False,
                    Condition::False => Condition::True,
                    Condition::Unknown => Condition::Unknown,
                });
            }
            if list.path.is_ident("all") {
                return Ok(if values.contains(&Condition::False) {
                    Condition::False
                } else if values.iter().all(|value| *value == Condition::True) {
                    Condition::True
                } else {
                    Condition::Unknown
                });
            }
            if list.path.is_ident("any") {
                return Ok(if values.contains(&Condition::True) {
                    Condition::True
                } else if values.iter().all(|value| *value == Condition::False) {
                    Condition::False
                } else {
                    Condition::Unknown
                });
            }
            Err("unsupported cfg predicate in production owner discovery".to_string())
        }
    }
}

fn effective_meta(meta: &Meta, output: &mut Vec<Meta>) -> Result<(), String> {
    if let Meta::List(list) = meta
        && list.path.is_ident("cfg_attr")
    {
        let values = nested(list)?;
        let (predicate, attributes) = values.split_first().ok_or("cfg_attr has no predicate")?;
        match condition(predicate)? {
            Condition::False => {}
            Condition::True => {
                for attribute in attributes {
                    effective_meta(attribute, output)?;
                }
            }
            Condition::Unknown => {
                if attributes.iter().any(|attribute| {
                    attribute.path().is_ident("path")
                        || attribute.path().is_ident("cfg")
                        || attribute.path().is_ident("cfg_attr")
                }) {
                    return Err("unresolved conditional module path or cfg attribute".to_string());
                }
            }
        }
    } else {
        output.push(meta.clone());
    }
    Ok(())
}

fn effective_attributes(attributes: &[Attribute]) -> Result<Vec<Meta>, String> {
    let mut output = Vec::new();
    for attribute in attributes {
        effective_meta(&attribute.meta, &mut output)?;
    }
    Ok(output)
}

pub(crate) fn production_attributes(attributes: &[Attribute]) -> Result<bool, String> {
    for meta in effective_attributes(attributes)? {
        if let Meta::List(list) = meta
            && list.path.is_ident("cfg")
        {
            let values = nested(&list)?;
            if values.len() != 1 {
                return Err("cfg must contain one predicate".to_string());
            }
            if condition(&values[0])? == Condition::False {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn item_attributes(item: &Item) -> Result<&[Attribute], String> {
    Ok(match item {
        Item::Const(value) => &value.attrs,
        Item::Enum(value) => &value.attrs,
        Item::ExternCrate(value) => &value.attrs,
        Item::Fn(value) => &value.attrs,
        Item::ForeignMod(value) => &value.attrs,
        Item::Impl(value) => &value.attrs,
        Item::Macro(value) => &value.attrs,
        Item::Mod(value) => &value.attrs,
        Item::Static(value) => &value.attrs,
        Item::Struct(value) => &value.attrs,
        Item::Trait(value) => &value.attrs,
        Item::TraitAlias(value) => &value.attrs,
        Item::Type(value) => &value.attrs,
        Item::Union(value) => &value.attrs,
        Item::Use(value) => &value.attrs,
        _ => return Err("unsupported item in production owner discovery".to_string()),
    })
}

fn retain_fields(fields: &mut syn::Fields) -> Result<(), String> {
    let values = match fields {
        syn::Fields::Named(fields) => &mut fields.named,
        syn::Fields::Unnamed(fields) => &mut fields.unnamed,
        syn::Fields::Unit => return Ok(()),
    };
    let mut retained = Punctuated::new();
    for field in std::mem::take(values) {
        if production_attributes(&field.attrs)? {
            retained.push(field);
        }
    }
    *values = retained;
    Ok(())
}

pub(crate) fn production_items(items: &[Item]) -> Result<Vec<Item>, String> {
    let mut retained = Vec::new();
    for item in items {
        if !production_attributes(item_attributes(item)?)? {
            continue;
        }
        let mut item = item.clone();
        match &mut item {
            Item::Impl(value) => {
                let mut members = Vec::new();
                for member in std::mem::take(&mut value.items) {
                    let attributes = match &member {
                        syn::ImplItem::Fn(item) => &item.attrs,
                        syn::ImplItem::Const(item) => &item.attrs,
                        syn::ImplItem::Type(item) => &item.attrs,
                        syn::ImplItem::Macro(item) => &item.attrs,
                        _ => return Err("unsupported production impl item".to_string()),
                    };
                    if production_attributes(attributes)? {
                        members.push(member);
                    }
                }
                value.items = members;
            }
            Item::Trait(value) => {
                let mut members = Vec::new();
                for member in std::mem::take(&mut value.items) {
                    let attributes = match &member {
                        syn::TraitItem::Fn(item) => &item.attrs,
                        syn::TraitItem::Const(item) => &item.attrs,
                        syn::TraitItem::Type(item) => &item.attrs,
                        syn::TraitItem::Macro(item) => &item.attrs,
                        _ => return Err("unsupported production trait item".to_string()),
                    };
                    if production_attributes(attributes)? {
                        members.push(member);
                    }
                }
                value.items = members;
            }
            Item::Struct(value) => retain_fields(&mut value.fields)?,
            Item::Enum(value) => {
                let mut variants = Punctuated::new();
                for mut variant in std::mem::take(&mut value.variants) {
                    if production_attributes(&variant.attrs)? {
                        retain_fields(&mut variant.fields)?;
                        variants.push(variant);
                    }
                }
                value.variants = variants;
            }
            Item::Mod(value) => {
                if let Some((_, items)) = &mut value.content {
                    *items = production_items(items)?;
                }
            }
            _ => {}
        }
        retained.push(item);
    }
    Ok(retained)
}

/// Follows the crate's native module declarations without filename-based exclusions.
pub fn discover_ledger_owners(entry: &Path) -> Result<Vec<LedgerOwnerModule>, String> {
    let entry = entry
        .canonicalize()
        .map_err(|error| format!("resolve {}: {error}", entry.display()))?;
    let root = entry
        .parent()
        .ok_or("owner entry has no parent")?
        .to_path_buf();
    let mut discovery = Discovery {
        root,
        parsed: HashSet::new(),
        modules: Vec::new(),
    };
    discovery.file(&entry, "crate".to_string(), entry.parent().unwrap())?;
    if discovery.modules.is_empty()
        || discovery
            .modules
            .iter()
            .all(|module| module.items.is_empty())
    {
        return Err("production owner collection is empty".to_string());
    }
    resolve_module_aliases(&mut discovery.modules)?;
    Ok(discovery.modules)
}

struct Discovery {
    root: PathBuf,
    parsed: HashSet<PathBuf>,
    modules: Vec<LedgerOwnerModule>,
}

impl Discovery {
    fn file(&mut self, path: &Path, module: String, directory: &Path) -> Result<(), String> {
        let path = path
            .canonicalize()
            .map_err(|error| format!("resolve owner {}: {error}", path.display()))?;
        if !path.starts_with(&self.root) {
            return Err(format!("owner {} escapes crate source", path.display()));
        }
        if !self.parsed.insert(path.clone()) {
            return Err(format!("duplicate owner parse {}", path.display()));
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("read owner {}: {error}", path.display()))?;
        if source.trim().is_empty() {
            return Err(format!("empty owner source {}", path.display()));
        }
        let parsed = syn::parse_file(&source)
            .map_err(|error| format!("parse owner {}: {error}", path.display()))?;
        if !production_attributes(&parsed.attrs)? {
            return Ok(());
        }
        self.scope(
            &path,
            module,
            directory,
            path.parent().ok_or("owner path has no parent")?,
            &parsed.items,
        )
    }

    fn scope(
        &mut self,
        path: &Path,
        module: String,
        directory: &Path,
        path_base: &Path,
        items: &[Item],
    ) -> Result<(), String> {
        if self.modules.iter().any(|owner| owner.module == module) {
            return Err(format!("duplicate production module {module}"));
        }
        let items = production_items(items)?;
        let mut local = Vec::new();
        let mut children = Vec::new();
        for item in items {
            if let Item::Mod(child) = item {
                children.push(child);
            } else if matches!(item, Item::Macro(_)) {
                return Err(format!(
                    "unresolved item macro in production owner {module}"
                ));
            } else {
                local.push(item);
            }
        }
        let aliases = local_type_aliases(&local);
        self.modules.push(LedgerOwnerModule {
            path: path.to_path_buf(),
            module: module.clone(),
            items: local,
            aliases,
        });
        for child in children {
            let child_name = format!("{module}::{}", child.ident);
            let mut paths = Vec::new();
            for meta in effective_attributes(&child.attrs)? {
                if meta.path().is_ident("path") {
                    let Meta::NameValue(value) = meta else {
                        return Err(format!("unsupported path attribute on {child_name}"));
                    };
                    let syn::Expr::Lit(value) = value.value else {
                        return Err(format!("nonliteral path on {child_name}"));
                    };
                    let syn::Lit::Str(value) = value.lit else {
                        return Err(format!("non-string path on {child_name}"));
                    };
                    paths.push(value.value());
                }
            }
            if paths.len() > 1 {
                return Err(format!("multiple path attributes on {child_name}"));
            }
            if let Some((_, items)) = child.content {
                if !paths.is_empty() {
                    return Err(format!("unsupported path on inline module {child_name}"));
                }
                self.scope(
                    path,
                    child_name,
                    &directory.join(child.ident.to_string()),
                    &directory.join(child.ident.to_string()),
                    &items,
                )?;
            } else {
                let file = if let Some(explicit) = paths.first() {
                    path_base.join(explicit)
                } else {
                    let direct = directory.join(format!("{}.rs", child.ident));
                    let nested = directory.join(child.ident.to_string()).join("mod.rs");
                    match (direct.is_file(), nested.is_file()) {
                        (true, false) => direct,
                        (false, true) => nested,
                        (false, false) => {
                            return Err(format!("missing module source for {child_name}"));
                        }
                        (true, true) => {
                            return Err(format!("ambiguous module sources for {child_name}"));
                        }
                    }
                };
                let child_directory = if file.file_name().is_some_and(|name| name == "mod.rs") {
                    file.parent()
                        .ok_or("module file has no parent")?
                        .to_path_buf()
                } else {
                    file.with_extension("")
                };
                self.file(&file, child_name, &child_directory)?;
            }
        }
        Ok(())
    }
}

fn imports(
    prefix: &mut Vec<String>,
    tree: &UseTree,
    output: &mut Vec<(Vec<String>, Option<String>)>,
) {
    match tree {
        UseTree::Path(value) => {
            prefix.push(value.ident.to_string());
            imports(prefix, &value.tree, output);
            prefix.pop();
        }
        UseTree::Name(value) => {
            let mut path = prefix.clone();
            path.push(value.ident.to_string());
            output.push((path, Some(value.ident.to_string())));
        }
        UseTree::Rename(value) => {
            let mut path = prefix.clone();
            path.push(value.ident.to_string());
            output.push((path, Some(value.rename.to_string())));
        }
        UseTree::Group(value) => {
            for item in &value.items {
                imports(prefix, item, output);
            }
        }
        UseTree::Glob(_) => output.push((prefix.clone(), None)),
    }
}

fn import_module(current: &str, segments: &[String]) -> String {
    let mut scope = current.split("::").map(str::to_string).collect::<Vec<_>>();
    for segment in segments {
        match segment.as_str() {
            "crate" => scope.truncate(1),
            "self" => {}
            "super" => {
                scope.pop();
            }
            _ => scope.push(segment.clone()),
        }
    }
    scope.join("::")
}

pub(super) fn resolve_module_aliases(modules: &mut [LedgerOwnerModule]) -> Result<(), String> {
    let mut bindings = Vec::new();
    for module in modules.iter() {
        let mut module_imports = Vec::new();
        for item in &module.items {
            if let Item::Use(item) = item {
                imports(&mut Vec::new(), &item.tree, &mut module_imports);
            } else if let Item::Type(item) = item
                && let syn::Type::Path(target) = item.ty.as_ref()
                && target.qself.is_none()
                && target
                    .path
                    .segments
                    .iter()
                    .all(|segment| matches!(segment.arguments, syn::PathArguments::None))
            {
                module_imports.push((
                    target
                        .path
                        .segments
                        .iter()
                        .map(|segment| segment.ident.to_string())
                        .collect(),
                    Some(item.ident.to_string()),
                ));
            }
        }
        bindings.push(module_imports);
    }
    for _ in 0..=modules.len() * 2 {
        let previous = modules
            .iter()
            .map(|module| (module.module.clone(), module.aliases.names.clone()))
            .collect::<HashMap<_, _>>();
        let mut changed = false;
        for (module, imports) in modules.iter_mut().zip(&bindings) {
            let mut names = local_type_aliases(&module.items).names;
            let explicit = names.keys().cloned().collect::<HashSet<_>>();
            for (path, binding) in imports {
                let (parent, target) = if binding.is_some() {
                    (&path[..path.len() - 1], path.last())
                } else {
                    (path.as_slice(), None)
                };
                let owner = import_module(&module.module, parent);
                if let Some(source) = previous.get(&owner) {
                    if let (Some(binding), Some(target)) = (binding, target) {
                        let aliases = LocalTypeAliases {
                            names: source.clone(),
                        };
                        names.insert(binding.clone(), resolve_alias(target, &aliases));
                    } else {
                        let aliases = LocalTypeAliases {
                            names: source.clone(),
                        };
                        for name in source.keys() {
                            if explicit.contains(name) {
                                continue;
                            }
                            let target = resolve_alias(name, &aliases);
                            if names.get(name).is_some_and(|previous| previous != &target) {
                                return Err(format!(
                                    "ambiguous glob alias {name} in {}",
                                    module.module
                                ));
                            }
                            names.insert(name.clone(), target);
                        }
                    }
                } else if binding.is_none() {
                    return Err(format!(
                        "unresolved glob import {} in {}",
                        path.join("::"),
                        module.module
                    ));
                }
            }
            changed |= names != module.aliases.names;
            module.aliases.names = names;
        }
        if !changed {
            return Ok(());
        }
    }
    Err("production module aliases did not converge".to_string())
}
