// SPDX-License-Identifier: AGPL-3.0-only

//! Source-derived architecture guards for ActingCommand Runtime ownership rules.

use std::collections::{HashMap, HashSet, VecDeque};

use syn::visit::Visit;
use syn::{
    BinOp, Expr, ExprMatch, FnArg, Item, ItemFn, Lit, Pat, ReturnType, Stmt, Type, UseTree,
    Visibility,
};

/// Top-level dispatch arms and the concrete commands they currently expose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInventory {
    pub dispatch_arm_count: usize,
    pub dispatch_arms: Vec<String>,
    pub commands: Vec<String>,
}

/// Enforces the exact line baseline so growth and unrecorded shrinkage both fail.
pub fn validate_line_ratchet(baseline: usize, actual: usize) -> Result<(), String> {
    if actual > baseline {
        return Err(format!(
            "apps/actinglab/src/main.rs grew from {baseline} to {actual} lines"
        ));
    }
    if actual < baseline {
        return Err(format!(
            "apps/actinglab/src/main.rs is {actual} lines; lower the ratchet from {baseline} in the same commit"
        ));
    }
    Ok(())
}

/// Finds CLI/process/config access forbidden inside the future `crates/lab` source tree.
pub fn inspect_lab_source(path: &str, source: &str) -> Result<Vec<String>, String> {
    syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;

    let checks = [
        ("FlagArgs", "FlagArgs"),
        ("process::exit", "process::exit"),
        ("env::var(", "env::var"),
        ("env::var_os(", "env::var_os"),
        ("env::temp_dir(", "env::temp_dir"),
        ("env::current_dir(", "env::current_dir"),
        (
            "pub fn package_build_pack(",
            "out-of-scope Lab::package_build_pack",
        ),
        (
            "pub fn compile_maa_tasks(",
            "out-of-scope Lab::compile_maa_tasks",
        ),
        ("println!(", "println!"),
        ("eprintln!(", "eprintln!"),
    ];
    Ok(checks
        .into_iter()
        .filter(|(needle, _)| source.contains(needle))
        .map(|(_, label)| format!("{path}: forbidden {label}"))
        .collect())
}

/// Finds public APIs that expose `serde_json::Value`, including imported aliases.
pub fn inspect_public_api(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut violations = Vec::new();
    inspect_public_items(path, &file.items, None, &mut violations);
    Ok(violations)
}

/// Validates the contract crate's declared package dependencies against its fixed budget.
pub fn contract_dependency_violations(manifest: &str) -> Result<Vec<String>, String> {
    let document = toml::from_str::<toml::Value>(manifest)
        .map_err(|err| format!("failed to parse contract Cargo.toml: {err}"))?;
    let mut dependencies = HashSet::new();
    collect_dependency_names(&document, None, &mut dependencies);

    let allowed = HashSet::from([
        "serde".to_string(),
        "serde_json".to_string(),
        "thiserror".to_string(),
    ]);
    let mut violations = dependencies
        .difference(&allowed)
        .map(|name| format!("unapproved contract dependency: {name}"))
        .collect::<Vec<_>>();
    violations.sort();
    Ok(violations)
}

/// Finds workspace dependency edges from a non-app package into an `apps/*` package.
pub fn workspace_dependency_violations(metadata: &str) -> Result<Vec<String>, String> {
    let document: serde_json::Value = serde_json::from_str(metadata)
        .map_err(|err| format!("failed to parse cargo metadata: {err}"))?;
    let packages = document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata is missing packages".to_string())?;
    let workspace_members = document
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata is missing workspace_members".to_string())?
        .iter()
        .map(required_string)
        .collect::<Result<HashSet<_>, _>>()?;

    let mut package_by_id = HashMap::new();
    for package in packages {
        let id = required_field_string(package, "id")?;
        let name = required_field_string(package, "name")?;
        let manifest_path = required_field_string(package, "manifest_path")?;
        let normalized_path = manifest_path.replace('\\', "/");
        package_by_id.insert(id, (name, normalized_path.contains("/apps/")));
    }

    let nodes = document
        .pointer("/resolve/nodes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata is missing resolve.nodes".to_string())?;
    let mut violations = Vec::new();
    for node in nodes {
        let id = required_field_string(node, "id")?;
        if !workspace_members.contains(&id) {
            continue;
        }
        let (package_name, is_app) = package_by_id
            .get(&id)
            .ok_or_else(|| format!("cargo metadata node has unknown package id {id}"))?;
        if *is_app {
            continue;
        }
        let dependencies = node
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("cargo metadata node {id} is missing dependencies"))?;
        for dependency in dependencies {
            let dependency_id = required_string(dependency)?;
            let Some((dependency_name, true)) = package_by_id.get(&dependency_id) else {
                continue;
            };
            violations.push(format!(
                "crate {package_name} depends on app {dependency_name}"
            ));
        }
    }
    violations.sort();
    violations.dedup();
    Ok(violations)
}

/// Finds direct or transitive dependency paths from production workspace packages to Lab.
pub fn lab_removability_violations(
    metadata: &str,
    optional_packages: &[&str],
) -> Result<Vec<String>, String> {
    let document: serde_json::Value = serde_json::from_str(metadata)
        .map_err(|err| format!("failed to parse cargo metadata: {err}"))?;
    let packages = document
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata is missing packages".to_string())?;
    let workspace_members = document
        .get("workspace_members")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata is missing workspace_members".to_string())?
        .iter()
        .map(required_string)
        .collect::<Result<Vec<_>, _>>()?;

    let mut package_names = HashMap::new();
    let mut lab_ids = Vec::new();
    for package in packages {
        let id = required_field_string(package, "id")?;
        let name = required_field_string(package, "name")?;
        if name == "actingcommand-lab" {
            lab_ids.push(id.clone());
        }
        package_names.insert(id, name);
    }
    if lab_ids.len() > 1 {
        return Err("cargo metadata contains multiple actingcommand-lab packages".to_string());
    }
    let Some(lab_id) = lab_ids.pop() else {
        return Ok(Vec::new());
    };

    let nodes = document
        .pointer("/resolve/nodes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata is missing resolve.nodes".to_string())?;
    let mut dependencies = HashMap::<String, Vec<String>>::new();
    for node in nodes {
        let id = required_field_string(node, "id")?;
        let node_dependencies = node
            .get("dependencies")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("cargo metadata node {id} is missing dependencies"))?
            .iter()
            .map(required_string)
            .collect::<Result<Vec<_>, _>>()?;
        dependencies.insert(id, node_dependencies);
    }

    let optional = optional_packages.iter().copied().collect::<HashSet<_>>();
    let mut violations = Vec::new();
    for root in workspace_members {
        let root_name = package_names
            .get(&root)
            .ok_or_else(|| format!("workspace member has unknown package id {root}"))?;
        if optional.contains(root_name.as_str()) {
            continue;
        }
        let Some(path) = dependency_path(&root, &lab_id, &dependencies) else {
            continue;
        };
        let names = path
            .iter()
            .map(|id| package_names.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect::<Vec<_>>();
        violations.push(format!(
            "production package {root_name} reaches actingcommand-lab: {}",
            names.join(" -> ")
        ));
    }
    violations.sort();
    Ok(violations)
}

fn dependency_path(
    root: &str,
    target: &str,
    dependencies: &HashMap<String, Vec<String>>,
) -> Option<Vec<String>> {
    let mut queue = VecDeque::from([vec![root.to_string()]]);
    let mut visited = HashSet::from([root.to_string()]);
    while let Some(path) = queue.pop_front() {
        let current = path.last()?;
        if current == target {
            return Some(path);
        }
        for dependency in dependencies.get(current).into_iter().flatten() {
            if !visited.insert(dependency.clone()) {
                continue;
            }
            let mut next = path.clone();
            next.push(dependency.clone());
            queue.push_back(next);
        }
    }
    None
}

/// Derives the command denominator from ActingLab's real dispatch AST.
pub fn extract_command_inventory(sources: &[(&str, &str)]) -> Result<CommandInventory, String> {
    let mut functions = HashMap::<String, Vec<ItemFn>>::new();
    for (path, source) in sources {
        let file =
            syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
        for item in file.items {
            if let Item::Fn(function) = item {
                let name = function.sig.ident.to_string();
                functions.entry(name).or_default().push(function);
            }
        }
    }

    let execute = unique_function(&functions, "execute")?;
    let dispatch = direct_match_expression(execute)
        .ok_or_else(|| "execute() must contain a direct dispatch match".to_string())?;
    let mut dispatch_arms = Vec::new();
    let mut commands = Vec::new();
    let mut dispatch_arm_count = 0;
    for arm in &dispatch.arms {
        if matches!(arm.pat, Pat::Wild(_)) {
            continue;
        }
        dispatch_arm_count += 1;
        let pattern_names = slice_pattern_names(&arm.pat)?;
        let guard = arm
            .guard
            .as_ref()
            .ok_or_else(|| "dispatch arm is missing a literal equality guard".to_string())?;
        let (guard_name, literal) = equality_guard(&guard.1)?;
        match pattern_names.as_slice() {
            [command_name] if guard_name == *command_name => {
                dispatch_arms.push(literal.clone());
                commands.push(literal);
            }
            [group_name, subcommand_name] if guard_name == *group_name => {
                dispatch_arms.push(format!("{literal} <subcommand>"));
                let callee = call_receiving_ident(&arm.body, subcommand_name)?;
                let function = unique_function(&functions, &callee).map_err(|err| {
                    format!("dispatch group '{literal}' cannot resolve '{callee}': {err}")
                })?;
                let helper_subcommand = first_argument_name(function)?;
                let subcommand_match =
                    match_on_ident(function, &helper_subcommand).ok_or_else(|| {
                        format!(
                            "dispatch function '{callee}' has no match on '{helper_subcommand}'"
                        )
                    })?;
                let subcommands = literal_patterns(subcommand_match)?;
                if subcommands.is_empty() {
                    return Err(format!(
                        "dispatch group '{literal}' has no concrete subcommands"
                    ));
                }
                commands.extend(
                    subcommands
                        .into_iter()
                        .map(|subcommand| format!("{literal} {subcommand}")),
                );
            }
            _ => {
                return Err(format!(
                    "dispatch guard '{guard_name} == {literal}' does not match its slice pattern"
                ));
            }
        }
    }

    let unique = commands.iter().collect::<HashSet<_>>();
    if unique.len() != commands.len() {
        return Err("ActingLab command inventory contains duplicate commands".to_string());
    }
    Ok(CommandInventory {
        dispatch_arm_count,
        dispatch_arms,
        commands,
    })
}

fn first_argument_name(function: &ItemFn) -> Result<String, String> {
    let Some(first) = function.sig.inputs.first() else {
        return Err(format!(
            "function '{}' has no arguments",
            function.sig.ident
        ));
    };
    let FnArg::Typed(argument) = first else {
        return Err(format!(
            "function '{}' starts with a receiver instead of a subcommand",
            function.sig.ident
        ));
    };
    let Pat::Ident(pattern) = argument.pat.as_ref() else {
        return Err(format!(
            "function '{}' first argument is not an identifier",
            function.sig.ident
        ));
    };
    Ok(pattern.ident.to_string())
}

fn unique_function<'a>(
    functions: &'a HashMap<String, Vec<ItemFn>>,
    name: &str,
) -> Result<&'a ItemFn, String> {
    match functions.get(name).map(Vec::as_slice) {
        Some([function]) => Ok(function),
        Some(functions) => Err(format!(
            "function '{name}' is ambiguous across {} source files",
            functions.len()
        )),
        None => Err(format!("ActingLab source is missing {name}()")),
    }
}

fn direct_match_expression(function: &ItemFn) -> Option<&ExprMatch> {
    function
        .block
        .stmts
        .iter()
        .find_map(|statement| match statement {
            Stmt::Expr(Expr::Match(expression), _) => Some(expression),
            _ => None,
        })
}

fn slice_pattern_names(pattern: &Pat) -> Result<Vec<String>, String> {
    let Pat::Slice(slice) = pattern else {
        return Err("dispatch arm must use a slice pattern".to_string());
    };
    slice
        .elems
        .iter()
        .map(|element| match element {
            Pat::Ident(ident) => Ok(ident.ident.to_string()),
            _ => Err("dispatch slice pattern must contain identifiers".to_string()),
        })
        .collect()
}

fn equality_guard(expression: &Expr) -> Result<(String, String), String> {
    let Expr::Binary(binary) = expression else {
        return Err("dispatch guard must be a binary equality".to_string());
    };
    if !matches!(binary.op, BinOp::Eq(_)) {
        return Err("dispatch guard must use ==".to_string());
    }
    path_and_string(&binary.left, &binary.right)
        .or_else(|| path_and_string(&binary.right, &binary.left))
        .ok_or_else(|| {
            "dispatch guard must compare an identifier with a string literal".to_string()
        })
}

fn path_and_string(path: &Expr, literal: &Expr) -> Option<(String, String)> {
    let Expr::Path(path) = path else {
        return None;
    };
    let ident = path.path.get_ident()?.to_string();
    let Expr::Lit(literal) = literal else {
        return None;
    };
    let Lit::Str(literal) = &literal.lit else {
        return None;
    };
    Some((ident, literal.value()))
}

fn call_receiving_ident(expression: &Expr, argument_name: &str) -> Result<String, String> {
    let mut visitor = CallFinder {
        argument_name,
        callees: Vec::new(),
    };
    visitor.visit_expr(expression);
    visitor.callees.sort();
    visitor.callees.dedup();
    match visitor.callees.as_slice() {
        [callee] => Ok(callee.clone()),
        [] => Err(format!(
            "dispatch group body does not call a function with '{argument_name}'"
        )),
        callees => Err(format!(
            "dispatch group body has ambiguous callees for '{argument_name}': {}",
            callees.join(", ")
        )),
    }
}

struct CallFinder<'a> {
    argument_name: &'a str,
    callees: Vec<String>,
}

impl<'ast> Visit<'ast> for CallFinder<'_> {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        let receives_argument = call.args.iter().any(|argument| {
            matches!(argument, Expr::Path(path) if path.path.get_ident().is_some_and(|ident| ident == self.argument_name))
        });
        if receives_argument
            && let Expr::Path(path) = call.func.as_ref()
            && let Some(segment) = path.path.segments.last()
        {
            self.callees.push(segment.ident.to_string());
        }
        syn::visit::visit_expr_call(self, call);
    }
}

fn match_on_ident<'a>(function: &'a ItemFn, ident: &str) -> Option<&'a ExprMatch> {
    let mut finder = MatchFinder { ident, found: None };
    finder.visit_block(&function.block);
    finder.found
}

struct MatchFinder<'needle, 'syntax> {
    ident: &'needle str,
    found: Option<&'syntax ExprMatch>,
}

impl<'ast> Visit<'ast> for MatchFinder<'_, 'ast> {
    fn visit_expr_match(&mut self, expression: &'ast ExprMatch) {
        let matches_ident = matches!(expression.expr.as_ref(), Expr::Path(path) if path.path.get_ident().is_some_and(|ident| ident == self.ident));
        if self.found.is_none() && matches_ident {
            self.found = Some(expression);
            return;
        }
        syn::visit::visit_expr_match(self, expression);
    }
}

fn literal_patterns(expression: &ExprMatch) -> Result<Vec<String>, String> {
    let mut literals = Vec::new();
    for arm in &expression.arms {
        if matches!(arm.pat, Pat::Wild(_) | Pat::Ident(_)) && arm.guard.is_none() {
            continue;
        }
        if arm.guard.is_some() {
            return Err("subcommand match contains a guarded dynamic pattern".to_string());
        }
        collect_pattern_literals(&arm.pat, &mut literals)?;
    }
    Ok(literals)
}

fn collect_pattern_literals(pattern: &Pat, literals: &mut Vec<String>) -> Result<(), String> {
    match pattern {
        Pat::Lit(pattern) => {
            let Lit::Str(literal) = &pattern.lit else {
                return Err("subcommand match contains a non-string literal".to_string());
            };
            literals.push(literal.value());
            Ok(())
        }
        Pat::Or(pattern) => {
            for case in &pattern.cases {
                collect_pattern_literals(case, literals)?;
            }
            Ok(())
        }
        _ => Err("subcommand match contains a non-literal pattern".to_string()),
    }
}

fn required_field_string(value: &serde_json::Value, field: &str) -> Result<String, String> {
    value
        .get(field)
        .ok_or_else(|| format!("cargo metadata entry is missing {field}"))
        .and_then(required_string)
}

fn required_string(value: &serde_json::Value) -> Result<String, String> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "cargo metadata value is not a string".to_string())
}

fn collect_dependency_names(
    value: &toml::Value,
    key: Option<&str>,
    dependencies: &mut HashSet<String>,
) {
    let Some(table) = value.as_table() else {
        return;
    };
    if key.is_some_and(|key| key == "dependencies" || key.ends_with("-dependencies")) {
        for (alias, specification) in table {
            let package = specification
                .as_table()
                .and_then(|table| table.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(alias);
            dependencies.insert(package.to_string());
        }
        return;
    }
    for (nested_key, nested_value) in table {
        collect_dependency_names(nested_value, Some(nested_key), dependencies);
    }
}

fn inspect_public_items(
    path: &str,
    items: &[Item],
    module: Option<&str>,
    violations: &mut Vec<String>,
) {
    let aliases = serde_json_value_aliases(items);
    let ledger_aliases = ledger_storage_aliases(items);
    for item in items {
        match item {
            Item::Fn(function) if is_public(&function.vis) => {
                if signature_uses_json_value(&function.sig, &aliases) {
                    violations.push(format!(
                        "{path}: public function {} uses serde_json::Value",
                        qualified(module, &function.sig.ident.to_string())
                    ));
                }
                if signature_uses_ledger_storage(&function.sig, &ledger_aliases) {
                    violations.push(format!(
                        "{path}: public function {} uses actingcommand_ledger storage types",
                        qualified(module, &function.sig.ident.to_string())
                    ));
                }
            }
            Item::Impl(item_impl) => {
                for impl_item in &item_impl.items {
                    let syn::ImplItem::Fn(method) = impl_item else {
                        continue;
                    };
                    if is_public(&method.vis) && signature_uses_json_value(&method.sig, &aliases) {
                        violations.push(format!(
                            "{path}: public method {} uses serde_json::Value",
                            qualified(module, &method.sig.ident.to_string())
                        ));
                    }
                    if is_public(&method.vis)
                        && signature_uses_ledger_storage(&method.sig, &ledger_aliases)
                    {
                        violations.push(format!(
                            "{path}: public method {} uses actingcommand_ledger storage types",
                            qualified(module, &method.sig.ident.to_string())
                        ));
                    }
                }
            }
            Item::Trait(item_trait) if is_public(&item_trait.vis) => {
                for trait_item in &item_trait.items {
                    let syn::TraitItem::Fn(method) = trait_item else {
                        continue;
                    };
                    if signature_uses_json_value(&method.sig, &aliases) {
                        let name = format!("{}::{}", item_trait.ident, method.sig.ident);
                        violations.push(format!(
                            "{path}: public trait method {} uses serde_json::Value",
                            qualified(module, &name)
                        ));
                    }
                    if signature_uses_ledger_storage(&method.sig, &ledger_aliases) {
                        let name = format!("{}::{}", item_trait.ident, method.sig.ident);
                        violations.push(format!(
                            "{path}: public trait method {} uses actingcommand_ledger storage types",
                            qualified(module, &name)
                        ));
                    }
                }
            }
            Item::Type(item_type)
                if is_public(&item_type.vis) && type_uses_json_value(&item_type.ty, &aliases) =>
            {
                violations.push(format!(
                    "{path}: public type alias {} points to serde_json::Value",
                    qualified(module, &item_type.ident.to_string())
                ));
            }
            Item::Type(item_type)
                if is_public(&item_type.vis)
                    && type_uses_ledger_storage(&item_type.ty, &ledger_aliases) =>
            {
                violations.push(format!(
                    "{path}: public type alias {} uses actingcommand_ledger storage types",
                    qualified(module, &item_type.ident.to_string())
                ));
            }
            Item::Struct(item_struct) if is_public(&item_struct.vis) => {
                for (index, field) in item_struct.fields.iter().enumerate() {
                    if !is_public(&field.vis)
                        || !type_uses_ledger_storage(&field.ty, &ledger_aliases)
                    {
                        continue;
                    }
                    let field_name = field
                        .ident
                        .as_ref()
                        .map_or_else(|| index.to_string(), ToString::to_string);
                    let name = format!("{}::{field_name}", item_struct.ident);
                    violations.push(format!(
                        "{path}: public field {} uses actingcommand_ledger storage types",
                        qualified(module, &name)
                    ));
                }
            }
            Item::Mod(item_mod) => {
                if let Some((_, nested)) = &item_mod.content {
                    let nested_name = qualified(module, &item_mod.ident.to_string());
                    inspect_public_items(path, nested, Some(&nested_name), violations);
                }
            }
            _ => {}
        }
    }
}

#[derive(Default)]
struct JsonValueAliases {
    values: HashSet<String>,
    modules: HashSet<String>,
}

const LEDGER_STORAGE_TYPES: &[&str] = &[
    "LabLedger",
    "LabLogError",
    "LabLogResult",
    "LastResortError",
    "LedgerRead",
    "LedgerRecord",
    "LedgerRecordKind",
    "LightEvent",
    "SessionHeader",
];

#[derive(Default)]
struct LedgerStorageAliases {
    types: HashSet<String>,
    modules: HashSet<String>,
}

fn ledger_storage_aliases(items: &[Item]) -> LedgerStorageAliases {
    let mut aliases = LedgerStorageAliases::default();
    for item in items {
        if let Item::Use(item_use) = item {
            collect_ledger_storage_alias(&mut Vec::new(), &item_use.tree, &mut aliases);
        }
    }
    aliases
}

fn collect_ledger_storage_alias(
    prefix: &mut Vec<String>,
    tree: &UseTree,
    aliases: &mut LedgerStorageAliases,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_ledger_storage_alias(prefix, &path.tree, aliases);
            prefix.pop();
        }
        UseTree::Name(name)
            if prefix == &["actingcommand_ledger"]
                && is_ledger_storage_type(&name.ident.to_string()) =>
        {
            aliases.types.insert(name.ident.to_string());
        }
        UseTree::Rename(rename)
            if prefix == &["actingcommand_ledger"]
                && is_ledger_storage_type(&rename.ident.to_string()) =>
        {
            aliases.types.insert(rename.rename.to_string());
        }
        UseTree::Rename(rename) if prefix.is_empty() && rename.ident == "actingcommand_ledger" => {
            aliases.modules.insert(rename.rename.to_string());
        }
        UseTree::Rename(rename)
            if prefix == &["actingcommand_ledger"] && rename.ident == "self" =>
        {
            aliases.modules.insert(rename.rename.to_string());
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_ledger_storage_alias(prefix, item, aliases);
            }
        }
        _ => {}
    }
}

fn signature_uses_ledger_storage(
    signature: &syn::Signature,
    aliases: &LedgerStorageAliases,
) -> bool {
    let input_uses_storage = signature.inputs.iter().any(|input| match input {
        FnArg::Receiver(_) => false,
        FnArg::Typed(argument) => type_uses_ledger_storage(&argument.ty, aliases),
    });
    let output_uses_storage = match &signature.output {
        ReturnType::Default => false,
        ReturnType::Type(_, output) => type_uses_ledger_storage(output, aliases),
    };
    input_uses_storage || output_uses_storage
}

fn type_uses_ledger_storage(value_type: &Type, aliases: &LedgerStorageAliases) -> bool {
    let mut visitor = LedgerStorageTypeVisitor {
        aliases,
        found: false,
    };
    visitor.visit_type(value_type);
    visitor.found
}

struct LedgerStorageTypeVisitor<'a> {
    aliases: &'a LedgerStorageAliases,
    found: bool,
}

impl<'ast> Visit<'ast> for LedgerStorageTypeVisitor<'_> {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        let segments = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        let direct = segments.first().is_some_and(|segment| {
            segment == "actingcommand_ledger" || self.aliases.modules.contains(segment)
        }) && segments
            .last()
            .is_some_and(|segment| is_ledger_storage_type(segment));
        let imported = segments.len() == 1 && self.aliases.types.contains(&segments[0]);
        if direct || imported {
            self.found = true;
        }
        syn::visit::visit_type_path(self, node);
    }
}

fn is_ledger_storage_type(candidate: &str) -> bool {
    LEDGER_STORAGE_TYPES.contains(&candidate)
}

fn serde_json_value_aliases(items: &[Item]) -> JsonValueAliases {
    let mut aliases = JsonValueAliases::default();
    for item in items {
        if let Item::Use(item_use) = item {
            collect_value_alias(&mut Vec::new(), &item_use.tree, &mut aliases);
        }
    }
    aliases
}

fn collect_value_alias(prefix: &mut Vec<String>, tree: &UseTree, aliases: &mut JsonValueAliases) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_value_alias(prefix, &path.tree, aliases);
            prefix.pop();
        }
        UseTree::Name(name) if prefix == &["serde_json"] && name.ident == "Value" => {
            aliases.values.insert(name.ident.to_string());
        }
        UseTree::Rename(rename) if prefix == &["serde_json"] && rename.ident == "Value" => {
            aliases.values.insert(rename.rename.to_string());
        }
        UseTree::Rename(rename) if prefix.is_empty() && rename.ident == "serde_json" => {
            aliases.modules.insert(rename.rename.to_string());
        }
        UseTree::Rename(rename) if prefix == &["serde_json"] && rename.ident == "self" => {
            aliases.modules.insert(rename.rename.to_string());
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_value_alias(prefix, item, aliases);
            }
        }
        _ => {}
    }
}

fn signature_uses_json_value(signature: &syn::Signature, aliases: &JsonValueAliases) -> bool {
    let input_uses_value = signature.inputs.iter().any(|input| match input {
        FnArg::Receiver(_) => false,
        FnArg::Typed(argument) => type_uses_json_value(&argument.ty, aliases),
    });
    let output_uses_value = match &signature.output {
        ReturnType::Default => false,
        ReturnType::Type(_, output) => type_uses_json_value(output, aliases),
    };
    input_uses_value || output_uses_value
}

fn type_uses_json_value(value_type: &Type, aliases: &JsonValueAliases) -> bool {
    let mut visitor = JsonValueTypeVisitor {
        aliases,
        found: false,
    };
    visitor.visit_type(value_type);
    visitor.found
}

struct JsonValueTypeVisitor<'a> {
    aliases: &'a JsonValueAliases,
    found: bool,
}

impl<'ast> Visit<'ast> for JsonValueTypeVisitor<'_> {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        let segments = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        let direct = segments.first().is_some_and(|segment| {
            segment == "serde_json" || self.aliases.modules.contains(segment)
        }) && segments.last().is_some_and(|segment| segment == "Value");
        let imported = segments.len() == 1 && self.aliases.values.contains(&segments[0]);
        if direct || imported {
            self.found = true;
        }
        syn::visit::visit_type_path(self, node);
    }
}

fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

fn qualified(module: Option<&str>, name: &str) -> String {
    module.map_or_else(|| name.to_string(), |module| format!("{module}::{name}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn contract_dependency_budget_resolves_renamed_packages() {
        let manifest = r#"
            [package]
            name = "actingcommand-contract"
            version = "0.1.0"

            [dependencies]
            json = { package = "serde_json", version = "1" }
            fake_serde = { package = "anyhow", version = "1" }
        "#;

        let violations = super::contract_dependency_violations(manifest).unwrap();

        assert_eq!(violations, vec!["unapproved contract dependency: anyhow"]);
    }

    #[test]
    fn line_ratchet_requires_exact_checked_in_count() {
        assert!(super::validate_line_ratchet(100, 100).is_ok());
        assert!(
            super::validate_line_ratchet(100, 101)
                .unwrap_err()
                .contains("grew")
        );
        assert!(
            super::validate_line_ratchet(100, 99)
                .unwrap_err()
                .contains("lower the ratchet")
        );
    }

    #[test]
    fn command_inventory_expands_group_dispatch_into_concrete_commands() {
        let source = r#"
            fn execute(invocation: &Invocation) {
                match invocation.command.as_slice() {
                    [cmd] if cmd == "help" => help(),
                    [group, sub] if group == "env" => run_env(sub),
                    _ => unknown(),
                }
            }

            fn run_env(sub: &str) {
                match sub {
                    "status" => status(),
                    "resolve" | "detect" => resolve_or_detect(),
                    _ => unknown(),
                }
            }
        "#;

        let inventory = super::extract_command_inventory(&[("main.rs", source)]).unwrap();

        assert_eq!(inventory.dispatch_arm_count, 2);
        assert_eq!(inventory.dispatch_arms, vec!["help", "env <subcommand>"]);
        assert_eq!(
            inventory.commands,
            vec!["help", "env status", "env resolve", "env detect"]
        );
    }

    #[test]
    fn workspace_dependency_guard_rejects_crate_to_app_edge() {
        let metadata = serde_json::json!({
            "packages": [
                {
                    "id": "app-id",
                    "name": "actingcommand-actinglab",
                    "manifest_path": "/repo/apps/actinglab/Cargo.toml"
                },
                {
                    "id": "crate-id",
                    "name": "actingcommand-lab",
                    "manifest_path": "/repo/crates/lab/Cargo.toml"
                }
            ],
            "workspace_members": ["app-id", "crate-id"],
            "resolve": {
                "nodes": [
                    {"id": "app-id", "dependencies": []},
                    {"id": "crate-id", "dependencies": ["app-id"]}
                ]
            }
        });

        let violations = super::workspace_dependency_violations(&metadata.to_string()).unwrap();

        assert_eq!(
            violations,
            vec!["crate actingcommand-lab depends on app actingcommand-actinglab"]
        );
    }

    #[test]
    fn lab_removability_guard_rejects_direct_and_transitive_production_dependencies() {
        let metadata = serde_json::json!({
            "packages": [
                {"id": "lab", "name": "actingcommand-lab"},
                {"id": "lab-cli", "name": "actingcommand-actinglab"},
                {"id": "bridge", "name": "runtime-bridge"},
                {"id": "direct", "name": "runtime-direct"},
                {"id": "transitive", "name": "runtime-transitive"},
                {"id": "clean", "name": "runtime-clean"}
            ],
            "workspace_members": ["lab", "lab-cli", "bridge", "direct", "transitive", "clean"],
            "resolve": {
                "nodes": [
                    {"id": "lab", "dependencies": []},
                    {"id": "lab-cli", "dependencies": ["lab"]},
                    {"id": "bridge", "dependencies": ["lab"]},
                    {"id": "direct", "dependencies": ["lab"]},
                    {"id": "transitive", "dependencies": ["bridge"]},
                    {"id": "clean", "dependencies": []}
                ]
            }
        });

        let violations = super::lab_removability_violations(
            &metadata.to_string(),
            &["actingcommand-lab", "actingcommand-actinglab"],
        )
        .unwrap();

        assert_eq!(
            violations,
            vec![
                "production package runtime-bridge reaches actingcommand-lab: runtime-bridge -> actingcommand-lab",
                "production package runtime-direct reaches actingcommand-lab: runtime-direct -> actingcommand-lab",
                "production package runtime-transitive reaches actingcommand-lab: runtime-transitive -> runtime-bridge -> actingcommand-lab"
            ]
        );
    }

    #[test]
    fn contract_dependency_budget_rejects_unapproved_dependency() {
        let manifest = r#"
            [package]
            name = "actingcommand-contract"
            version = "0.1.0"

            [dependencies]
            serde = "1"
            anyhow = "1"
        "#;

        let violations = super::contract_dependency_violations(manifest).unwrap();

        assert_eq!(violations, vec!["unapproved contract dependency: anyhow"]);
    }

    #[test]
    fn public_api_guard_detects_json_value_shapes() {
        let source = r#"
            use serde_json::Value as JsonValue;
            use serde_json as json;

            pub fn direct() -> serde_json::Value { unreachable!() }
            pub async fn aliased(input: JsonValue) { let _ = input; }
            pub fn module_alias() -> json::Value { unreachable!() }
            pub trait Port { fn carry(&self) -> JsonValue; }
            pub type Payload = JsonValue;
            fn private_helper() -> JsonValue { unreachable!() }
        "#;

        let violations = super::inspect_public_api("fixture.rs", source).unwrap();

        assert!(violations.iter().any(|item| item.contains("direct")));
        assert!(violations.iter().any(|item| item.contains("aliased")));
        assert!(violations.iter().any(|item| item.contains("module_alias")));
        assert!(violations.iter().any(|item| item.contains("Port::carry")));
        assert!(violations.iter().any(|item| item.contains("Payload")));
        assert!(
            !violations
                .iter()
                .any(|item| item.contains("private_helper"))
        );
    }

    #[test]
    fn public_api_guard_detects_ledger_storage_shapes() {
        let source = r#"
            use actingcommand_ledger::{LedgerRecord as StoredRecord, LedgerRead};
            use actingcommand_ledger::LastResortError;

            pub trait Port {
                fn append(&mut self, record: StoredRecord);
                fn read(&self) -> LedgerRead;
            }
            pub struct Request {
                pub header: actingcommand_ledger::SessionHeader,
                private: actingcommand_ledger::LightEvent,
            }
            impl Request {
                pub fn last_resort(error: LastResortError) { let _ = error; }
            }
            fn private_helper() -> actingcommand_ledger::LedgerRecord { unreachable!() }
        "#;

        let violations = super::inspect_public_api("fixture.rs", source).unwrap();

        assert!(violations.iter().any(|item| item.contains("Port::append")));
        assert!(violations.iter().any(|item| item.contains("Port::read")));
        assert!(
            violations
                .iter()
                .any(|item| item.contains("Request::header"))
        );
        assert!(violations.iter().any(|item| item.contains("last_resort")));
        assert!(
            !violations
                .iter()
                .any(|item| item.contains("private_helper"))
        );
    }

    #[test]
    fn source_guard_detects_forbidden_lab_tokens() {
        let source = r#"
            fn parse(flags: FlagArgs) {
                println!("{flags:?}");
                eprintln!("bad");
                std::process::exit(1);
                let _ = std::env::var("ACTINGCOMMAND_CONFIG");
                let _ = std::env::var_os("ACTINGCOMMAND_CONFIG");
                let _ = std::env::temp_dir();
                let _ = std::env::current_dir();
            }
            pub fn package_build_pack() {}
            pub fn compile_maa_tasks() {}
        "#;

        let violations = super::inspect_lab_source("fixture.rs", source).unwrap();

        assert!(violations.iter().any(|item| item.contains("FlagArgs")));
        assert!(violations.iter().any(|item| item.contains("println!")));
        assert!(violations.iter().any(|item| item.contains("eprintln!")));
        assert!(violations.iter().any(|item| item.contains("process::exit")));
        assert!(violations.iter().any(|item| item.contains("env::var")));
        assert!(violations.iter().any(|item| item.contains("env::var_os")));
        assert!(violations.iter().any(|item| item.contains("env::temp_dir")));
        assert!(
            violations
                .iter()
                .any(|item| item.contains("env::current_dir"))
        );
        assert!(
            violations
                .iter()
                .any(|item| item.contains("Lab::package_build_pack"))
        );
        assert!(
            violations
                .iter()
                .any(|item| item.contains("Lab::compile_maa_tasks"))
        );
    }
}
