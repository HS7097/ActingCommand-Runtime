// SPDX-License-Identifier: AGPL-3.0-only

//! Source-derived architecture guards for ActingCommand Runtime ownership rules.

use std::collections::{HashMap, HashSet, VecDeque};

mod ledger_owners;
pub use ledger_owners::{LedgerOwnerModule, discover_ledger_owners};

use proc_macro2::{Span, TokenStream, TokenTree};
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
        ("println!(", "println!"),
        ("eprintln!(", "eprintln!"),
    ];
    Ok(checks
        .into_iter()
        .filter(|(needle, _)| source.contains(needle))
        .map(|(_, label)| format!("{path}: forbidden {label}"))
        .collect())
}

/// Finds writes to the process's stderr that Runtime-domain crates must not contain:
/// `eprintln!` / `eprint!` invocations (also when nested inside another macro's body),
/// `io::stderr(` calls and `io::Stderr` / `io::StderrLock` handles under any `io` module
/// (`std::io` and `tokio::io` alike), `use` imports of those names, and a bare `Stderr` /
/// `StderrLock` type path. Items behind `#[cfg(test)]` and the body of an inline `mod tests`
/// are skipped; which files count as test files is the caller's decision. Each violation is
/// reported as `path:line`.
pub fn inspect_stderr_writes(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let mut visitor = StderrWriteVisitor {
        path,
        violations: Vec::new(),
    };
    for item in &items {
        visitor.visit_item(item);
    }
    Ok(visitor.violations)
}

struct StderrWriteVisitor<'a> {
    path: &'a str,
    violations: Vec<String>,
}

impl StderrWriteVisitor<'_> {
    fn record(&mut self, span: Span, label: &str) {
        self.violations.push(format!(
            "{}:{} writes to stderr via {label}",
            self.path,
            span.start().line
        ));
    }

    /// Macro bodies are opaque to `syn`, so their tokens are scanned for the same needles.
    fn scan_macro_tokens(&mut self, tokens: TokenStream) {
        let mut flat = Vec::new();
        flatten_tokens(tokens, &mut flat);
        for (index, token) in flat.iter().enumerate() {
            let TokenTree::Ident(ident) = token else {
                continue;
            };
            let name = ident.to_string();
            if matches!(name.as_str(), "eprintln" | "eprint")
                && matches!(flat.get(index + 1), Some(TokenTree::Punct(punct)) if punct.as_char() == '!')
            {
                self.record(ident.span(), &format!("{name}!"));
            }
            if name == "io"
                && let (Some(TokenTree::Punct(first)), Some(TokenTree::Punct(second))) =
                    (flat.get(index + 1), flat.get(index + 2))
                && first.as_char() == ':'
                && second.as_char() == ':'
                && let Some(TokenTree::Ident(target)) = flat.get(index + 3)
                && is_stderr_handle(&target.to_string())
            {
                self.record(target.span(), &format!("io::{target}"));
            }
        }
    }
}

fn flatten_tokens(tokens: TokenStream, flat: &mut Vec<TokenTree>) {
    for token in tokens {
        match token {
            TokenTree::Group(group) => flatten_tokens(group.stream(), flat),
            other => flat.push(other),
        }
    }
}

fn is_stderr_handle(name: &str) -> bool {
    matches!(name, "stderr" | "Stderr" | "StderrLock")
}

fn collect_use_paths(
    prefix: &mut Vec<String>,
    tree: &UseTree,
    paths: &mut Vec<(Vec<String>, Span)>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_paths(prefix, &path.tree, paths);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut full = prefix.clone();
            full.push(name.ident.to_string());
            paths.push((full, name.ident.span()));
        }
        UseTree::Rename(rename) => {
            let mut full = prefix.clone();
            full.push(rename.ident.to_string());
            paths.push((full, rename.ident.span()));
        }
        UseTree::Glob(_) => {}
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_paths(prefix, item, paths);
            }
        }
    }
}

impl<'ast> Visit<'ast> for StderrWriteVisitor<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        // `production_items` already dropped `#[cfg(test)]` items; an inline `mod tests` is test
        // scaffolding by convention even without the attribute.
        if let Item::Mod(module) = item
            && module.ident == "tests"
        {
            return;
        }
        syn::visit::visit_item(self, item);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let mut paths = Vec::new();
        collect_use_paths(&mut Vec::new(), &item.tree, &mut paths);
        for (segments, span) in paths {
            if let [.., parent, last] = segments.as_slice()
                && parent == "io"
                && is_stderr_handle(last)
            {
                self.record(span, &format!("use of io::{last}"));
            }
        }
        syn::visit::visit_item_use(self, item);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if let Some(segment) = mac.path.segments.last()
            && matches!(segment.ident.to_string().as_str(), "eprintln" | "eprint")
        {
            self.record(segment.ident.span(), &format!("{}!", segment.ident));
        }
        self.scan_macro_tokens(mac.tokens.clone());
        syn::visit::visit_macro(self, mac);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        let segments = path.segments.iter().collect::<Vec<_>>();
        match segments.as_slice() {
            [.., parent, last]
                if parent.ident == "io" && is_stderr_handle(&last.ident.to_string()) =>
            {
                self.record(last.ident.span(), &format!("io::{}", last.ident));
            }
            [only] if matches!(only.ident.to_string().as_str(), "Stderr" | "StderrLock") => {
                self.record(only.ident.span(), &only.ident.to_string());
            }
            _ => {}
        }
        syn::visit::visit_path(self, path);
    }
}

/// Rejects project identities from Runtime-owned code, contracts, defaults, and fixtures.
pub fn inspect_generic_runtime_identity(path: &str, source: &str) -> Vec<String> {
    const FORBIDDEN_LITERALS: &[&str] = &[
        "maa.",
        "com.yostar",
        "com.bilibili",
        "com.hypergryph",
        "\u{51fa}\u{6483}",
        "\u{6226}\u{95d8}",
        "\u{5efa}\u{9020}",
        "\u{9000}\u{5f79}",
        "\u{5f37}\u{5316}",
    ];
    const FORBIDDEN_WORDS: &[&str] = &[
        "ak",
        "alas",
        "ark",
        "arknights",
        "azur",
        "azurlane",
        "ba",
        "baas",
        "bluearchive",
        "gacha",
        "originite",
        "pvp",
        "pyroxene",
        "sortie",
    ];
    const FORBIDDEN_SEQUENCES: &[&[&str]] = &[
        &["blue", "archive"],
        &["server", "cn"],
        &["server", "jp"],
        &["server", "ko"],
        &["server", "maa"],
        &["server", "tw"],
    ];
    const FORBIDDEN_STANDALONE_WORDS: &[&str] = &["cn", "jp", "ko", "tw"];

    let normalized_path = path.replace('\\', "/").to_ascii_lowercase();
    let mut violations = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let lower = line.to_ascii_lowercase();
        for token in FORBIDDEN_LITERALS {
            if !lower.contains(token) {
                continue;
            }
            // This filename is an external provider artifact, not a Runtime identity or branch.
            if *token == "maa."
                && normalized_path.starts_with("crates/vision-ffi/")
                && lower.contains("external-tools/vision/fastdeploy/fastdeploy_ppocr_maa.dll")
            {
                continue;
            }
            violations.push(format!(
                "{path}:{} contains project-specific token {token}",
                index + 1
            ));
        }
        let words = identifier_words(line);
        for sequence in FORBIDDEN_SEQUENCES {
            if contains_word_sequence(&words, sequence) {
                violations.push(format!(
                    "{path}:{} contains project-specific token {}",
                    index + 1,
                    sequence.join("_")
                ));
            }
        }
        for word in &words {
            if FORBIDDEN_WORDS.contains(&word.as_str()) {
                violations.push(format!(
                    "{path}:{} contains project-specific word {word}",
                    index + 1
                ));
            }
        }
        for word in
            lower.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        {
            if FORBIDDEN_STANDALONE_WORDS.contains(&word) {
                violations.push(format!(
                    "{path}:{} contains project-specific word {word}",
                    index + 1
                ));
            }
        }
    }
    violations
}

/// Rejects built-in game identities from product and resource-authoring code.
pub fn inspect_generic_authoring_identity(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut visitor = GenericAuthoringIdentityVisitor {
        path,
        violations: Vec::new(),
    };
    visitor.visit_file(&file);
    Ok(visitor.violations)
}

struct GenericAuthoringIdentityVisitor<'a> {
    path: &'a str,
    violations: Vec<String>,
}

impl Visit<'_> for GenericAuthoringIdentityVisitor<'_> {
    fn visit_item_mod(&mut self, node: &syn::ItemMod) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_fn(&mut self, node: &syn::ItemFn) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.sig.ident);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &syn::ImplItemFn) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.sig.ident);
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_item_struct(&mut self, node: &syn::ItemStruct) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &syn::ItemEnum) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_union(&mut self, node: &syn::ItemUnion) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_union(self, node);
    }

    fn visit_item_trait(&mut self, node: &syn::ItemTrait) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_trait(self, node);
    }

    fn visit_item_type(&mut self, node: &syn::ItemType) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_type(self, node);
    }

    fn visit_item_const(&mut self, node: &syn::ItemConst) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_const(self, node);
    }

    fn visit_item_static(&mut self, node: &syn::ItemStatic) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        self.record_identifier(&node.ident);
        syn::visit::visit_item_static(self, node);
    }

    fn visit_lit_str(&mut self, node: &syn::LitStr) {
        let value = node.value();
        if contains_forbidden_authoring_identity(&value) {
            self.violations.push(format!(
                "{}: production code contains built-in game identity {value:?}",
                self.path
            ));
        }
    }

    fn visit_lit_byte_str(&mut self, node: &syn::LitByteStr) {
        let value = node.value();
        if contains_forbidden_authoring_identity(&String::from_utf8_lossy(&value)) {
            self.violations.push(format!(
                "{}: production code contains built-in game identity in byte string {value:?}",
                self.path
            ));
        }
    }

    fn visit_ident(&mut self, node: &syn::Ident) {
        self.record_identifier(node);
    }
}

impl GenericAuthoringIdentityVisitor<'_> {
    fn record_identifier(&mut self, identifier: &syn::Ident) {
        let value = identifier.to_string();
        if contains_forbidden_authoring_identity(&value) {
            let violation = format!(
                "{}: production code contains built-in game identifier {value}",
                self.path
            );
            if !self.violations.contains(&violation) {
                self.violations.push(violation);
            }
        }
    }
}

fn contains_forbidden_authoring_identity(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    let words = identifier_words(value.trim());
    is_forbidden_authoring_identity(&normalized)
        || words
            .iter()
            .any(|word| is_forbidden_authoring_identity(word))
        || contains_word_sequence(&words, &["blue", "archive"])
}

fn is_forbidden_authoring_identity(value: &str) -> bool {
    matches!(
        value,
        "ak" | "al"
            | "ark"
            | "arknights"
            | "azur"
            | "azurlane"
            | "azur_lane"
            | "ba"
            | "bluearchive"
            | "blue_archive"
            | "maaassistantarknights"
    )
}

fn identifier_words(value: &str) -> Vec<String> {
    let characters = value.chars().collect::<Vec<_>>();
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous: Option<char> = None;

    for (index, character) in characters.iter().copied().enumerate() {
        if !character.is_ascii_alphanumeric() {
            push_identifier_word(&mut words, &mut current);
            previous = None;
            continue;
        }
        let next = characters.get(index + 1).copied();
        let starts_word = previous.is_some_and(|previous| {
            character.is_ascii_uppercase()
                && (previous.is_ascii_lowercase()
                    || previous.is_ascii_digit()
                    || previous.is_ascii_uppercase()
                        && next.is_some_and(|next| next.is_ascii_lowercase()))
        });
        if starts_word {
            push_identifier_word(&mut words, &mut current);
        }
        current.push(character.to_ascii_lowercase());
        previous = Some(character);
    }
    push_identifier_word(&mut words, &mut current);
    // Long identity names remain recognizable inside acronym or mixed-case runs.
    // Short codes still require an identifier word boundary.
    for token in value.split(|character: char| !character.is_ascii_alphanumeric()) {
        let lower = token.to_ascii_lowercase();
        for identity in [
            "arknights",
            "azurlane",
            "bluearchive",
            "gacha",
            "originite",
            "pyroxene",
            "sortie",
        ] {
            if lower.contains(identity) && !words.iter().any(|word| word == identity) {
                words.push(identity.to_string());
            }
        }
        if matches!(
            lower.as_str(),
            "ak" | "al" | "ark" | "ba" | "alas" | "azur" | "baas" | "pvp"
        ) && !words.contains(&lower)
        {
            words.push(lower.clone());
        }
        for suffix in ["cn", "jp", "ko", "maa", "tw"] {
            if lower.contains(&format!("server{suffix}"))
                && !contains_word_sequence(&words, &["server", suffix])
            {
                words.extend(["server".to_string(), suffix.to_string()]);
            }
        }
    }
    words
}

fn push_identifier_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}

fn contains_word_sequence(words: &[String], sequence: &[&str]) -> bool {
    words.windows(sequence.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(sequence.iter().copied())
    })
}

fn has_cfg_test(attributes: &[syn::Attribute]) -> bool {
    matches!(ledger_owners::production_attributes(attributes), Ok(false))
}

/// Finds public APIs that expose `serde_json::Value`, including imported aliases.
pub fn inspect_public_api(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut violations = Vec::new();
    inspect_public_items(path, &file.items, None, &mut violations);
    Ok(violations)
}

/// Checks Sanitized inputs, Ledger ownership and forwarding to the shared append request.
/// This structure check does not establish write counts or persistence/error semantics.
/// Observation adapter boundary: https://github.com/HS7097/ActingCommand-Workflow/issues/285#issuecomment-5654739712
pub fn inspect_global_append_ingress(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let aliases = local_type_aliases(&items);
    inspect_ledger_append_ingress(&[LedgerOwnerModule {
        path: path.into(),
        module: "crate".to_string(),
        items,
        aliases,
    }])
}

/// Checks the complete discovered Ledger owner set without merging lexical scopes.
pub fn inspect_ledger_append_ingress(owners: &[LedgerOwnerModule]) -> Result<Vec<String>, String> {
    if owners.is_empty() {
        return Err("empty Ledger owner collection".to_string());
    }
    let path = owners[0].path.display().to_string();
    let mut append_methods = Vec::new();
    let mut observation_methods = Vec::new();
    let mut request_methods = Vec::new();
    let mut alternate_ingress_methods = Vec::new();
    for owner in owners {
        let aliases = &owner.aliases;
        for item in &owner.items {
            let Item::Impl(item_impl) = item else {
                continue;
            };
            if impl_self_ident(item_impl)
                .is_none_or(|ident| resolve_alias(&ident.to_string(), aliases) != "GlobalLedger")
            {
                continue;
            }
            for item in &item_impl.items {
                let syn::ImplItem::Fn(method) = item else {
                    continue;
                };
                if method.sig.ident == "append" && is_public(&method.vis) {
                    append_methods.push((item_impl, method, owner));
                    continue;
                }
                if method.sig.ident == "append_with_observation" {
                    observation_methods.push((item_impl, method, owner));
                }
                if method.sig.ident == "append_request" {
                    request_methods.push((item_impl, method, owner));
                }
                if method.sig.ident == "append_transaction" && is_public(&method.vis) {
                    let typed = method
                        .sig
                        .inputs
                        .iter()
                        .filter_map(|input| match input {
                            FnArg::Receiver(_) => None,
                            FnArg::Typed(argument) => Some(argument),
                        })
                        .collect::<Vec<_>>();
                    let typed_work = typed.get(1).is_some_and(|argument| {
                    let Type::Path(path) = argument.ty.as_ref() else { return false; };
                    let Some(segment) = path.path.segments.last() else { return false; };
                    if segment.ident != "Box" { return false; }
                    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else { return false; };
                    if arguments.args.len() != 1 { return false; }
                    let Some(syn::GenericArgument::Type(Type::TraitObject(object))) = arguments.args.first() else { return false; };
                    object.dyn_token.is_some() && object.bounds.len() == 1 && matches!(object.bounds.first(),
                        Some(syn::TypeParamBound::Trait(bound)) if bound.path.segments.last().is_some_and(|segment| segment.ident == "LedgerTransactionWork"))
                });
                    if typed.len() == 2
                        && method.sig.generics.params.is_empty()
                        && pattern_ident(&typed[0].pat).is_some_and(|ident| ident == "draft")
                        && type_last_ident(&typed[0].ty)
                            .is_some_and(|ident| ident == "SanitizedEventDraft")
                        && pattern_ident(&typed[1].pat).is_some_and(|ident| ident == "work")
                        && typed_work
                    {
                        continue;
                    }
                }
                // Workflow #325 C-1: the deferred ingress takes the same sanitized draft as
                // `append`; only the reply wait differs.
                if method.sig.ident == "append_deferred" && is_public(&method.vis) {
                    let typed = method
                        .sig
                        .inputs
                        .iter()
                        .filter_map(|input| match input {
                            FnArg::Receiver(_) => None,
                            FnArg::Typed(argument) => Some(argument),
                        })
                        .collect::<Vec<_>>();
                    if typed.len() == 1
                        && method.sig.generics.params.is_empty()
                        && pattern_ident(&typed[0].pat).is_some_and(|ident| ident == "draft")
                        && type_last_ident(&typed[0].ty)
                            .is_some_and(|ident| ident == "SanitizedEventDraft")
                    {
                        continue;
                    }
                }
                if is_public(&method.vis)
                    && (method.sig.ident.to_string().starts_with("append")
                        || method_accepts_event_ingress(method)
                        || method.sig.inputs.iter().any(|input| {
                            let FnArg::Typed(argument) = input else {
                                return false;
                            };
                            [
                                "EventDraft",
                                "SanitizedEventDraft",
                                "EventPayloadDraft",
                                "ArtifactReference",
                                "PersistedEvent",
                            ]
                            .iter()
                            .any(|name| type_uses_resolved_ident(&argument.ty, name, aliases))
                        }))
                {
                    alternate_ingress_methods.push((method.sig.ident.to_string(), owner));
                }
            }
        }
    }

    let mut violations = Vec::new();
    if append_methods.len() != 1 {
        violations.push(format!(
            "{path}: expected exactly one public GlobalLedger::append, found {}",
            append_methods.len()
        ));
    } else {
        let method = append_methods[0].1;
        let aliases = &append_methods[0].2.aliases;
        let typed = method
            .sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                FnArg::Receiver(_) => None,
                FnArg::Typed(argument) => Some(argument),
            })
            .collect::<Vec<_>>();
        let exact = typed.len() == 1
            && resolved_type_ident(&typed[0].ty, aliases)
                .is_some_and(|ident| ident == "SanitizedEventDraft");
        if !exact {
            violations.push(format!(
                "{path}: GlobalLedger::append must accept one SanitizedEventDraft input"
            ));
        }
    }
    let mut observation_adapter_valid = false;
    if !observation_methods.is_empty() {
        let previous_violations = violations.len();
        if observation_methods.len() != 1 {
            violations.push(format!(
                "{path}: expected one GlobalLedger::append_with_observation adapter, found {}",
                observation_methods.len()
            ));
        }
        if request_methods.len() != 1 {
            violations.push(format!(
                "{path}: expected one private GlobalLedger::append_request, found {}",
                request_methods.len()
            ));
        }
        for (item_impl, method, owner) in observation_methods
            .iter()
            .chain(append_methods.iter())
            .chain(request_methods.iter())
        {
            let aliases = &owner.aliases;
            let path = owner.path.display();
            let request = method.sig.ident == "append_request";
            let typed = method
                .sig
                .inputs
                .iter()
                .filter_map(|input| match input {
                    FnArg::Receiver(_) => None,
                    FnArg::Typed(argument) => Some(argument),
                })
                .collect::<Vec<_>>();
            let owned = item_impl.trait_.is_none()
                && if request {
                    matches!(method.vis, Visibility::Inherited)
                } else {
                    matches!(method.vis, Visibility::Public(_))
                }
                && matches!(method.sig.inputs.first(), Some(FnArg::Receiver(receiver))
                    if matches!(receiver.ty.as_ref(), Type::Reference(reference)
                        if type_last_ident(&reference.elem)
                            .is_some_and(|ident| ident == "Self" || ident == "GlobalLedger")));
            let inputs_known = typed.len() == if request { 2 } else { 1 }
                && typed.first().is_some_and(|argument| {
                    resolved_type_ident(&argument.ty, aliases)
                        .is_some_and(|ident| ident == "SanitizedEventDraft")
                })
                && (!request || typed.get(1).is_some_and(|argument| {
                    resolved_type_ident(&argument.ty, aliases)
                        .is_some_and(|ident| ident == "bool")
                }))
                && typed.iter().all(|argument| {
                    !item_impl.generics.params.iter().chain(method.sig.generics.params.iter())
                        .any(|parameter| matches!(parameter, syn::GenericParam::Type(parameter)
                            if type_last_ident(&argument.ty).is_some_and(|ident| *ident == parameter.ident)))
                });
            if !owned || !inputs_known {
                violations.push(format!(
                    "{path}: GlobalLedger::{} has unresolved Ledger ownership or Sanitized append inputs",
                    method.sig.ident
                ));
                continue;
            }
            if request {
                continue;
            }
            let Some(draft_name) = typed
                .first()
                .and_then(|argument| pattern_ident(&argument.pat))
            else {
                violations.push(format!(
                    "{path}: GlobalLedger::{} Sanitized input binding is unresolved",
                    method.sig.ident
                ));
                continue;
            };
            // Follow only returned values and their direct local bindings in these
            // named adapters. Unsupported statements remain an explicit coverage gap.
            let mut bindings = HashMap::new();
            let mut blocks = vec![&method.block];
            let mut expressions = Vec::new();
            let mut forwarding_calls = 0;
            let mut gap = None;
            while gap.is_none() && (!blocks.is_empty() || !expressions.is_empty()) {
                if let Some(block) = blocks.pop() {
                    for (index, statement) in block.stmts.iter().enumerate() {
                        match statement {
                            Stmt::Local(local) => {
                                let mut pattern = &local.pat;
                                if let Pat::Type(typed) = pattern {
                                    pattern = &typed.pat;
                                }
                                // Destructuring the original result from its observation
                                // still forwards the same request. Result semantics are reviewed separately.
                                if let Pat::Tuple(tuple) = pattern
                                    && tuple.elems.len() == 2
                                    && matches!(tuple.elems.last(), Some(Pat::Wild(_)))
                                {
                                    pattern = &tuple.elems[0];
                                }
                                let Some(name) = pattern_ident(pattern) else {
                                    gap = Some("local binding pattern");
                                    break;
                                };
                                let Some(initializer) = &local.init else {
                                    gap = Some("local binding without an initializer");
                                    break;
                                };
                                if initializer.diverge.is_some()
                                    || name == draft_name
                                    || bindings
                                        .insert(name.to_string(), initializer.expr.as_ref())
                                        .is_some()
                                {
                                    gap = Some("shadowed or conditional local binding");
                                    break;
                                }
                            }
                            Stmt::Expr(expression, semi)
                                if index + 1 == block.stmts.len()
                                    && (semi.is_none()
                                        || matches!(expression, Expr::Return(_))) =>
                            {
                                expressions.push((expression, "returned value"));
                            }
                            _ => {
                                gap = Some("statement outside direct forwarding");
                                break;
                            }
                        }
                    }
                    continue;
                }
                let Some((expression, role)) = expressions.pop() else {
                    break;
                };
                match expression {
                    Expr::Paren(value) => expressions.push((&value.expr, role)),
                    Expr::Group(value) => expressions.push((&value.expr, role)),
                    Expr::Return(value) if role == "returned value" => {
                        if let Some(value) = &value.expr {
                            expressions.push((value, role));
                        } else {
                            gap = Some("return without a forwarded value");
                        }
                    }
                    Expr::Block(value) if role == "returned value" => blocks.push(&value.block),
                    Expr::Field(value) if role == "returned value" => {
                        expressions.push((&value.base, role))
                    }
                    Expr::Path(value) if value.qself.is_none() => {
                        let Some(name) = value.path.get_ident() else {
                            gap = Some(role);
                            continue;
                        };
                        if let Some(initializer) = bindings.remove(&name.to_string()) {
                            expressions.push((initializer, role));
                        } else if !((role == "receiver" && name == "self")
                            || (role == "Sanitized input" && name == draft_name))
                        {
                            gap = Some(role);
                        }
                    }
                    Expr::Lit(value)
                        if role == "observation flag" && matches!(value.lit, Lit::Bool(_)) => {}
                    Expr::Unary(value)
                        if role == "observation flag" && matches!(value.op, syn::UnOp::Not(_)) =>
                    {
                        expressions.push((&value.expr, role));
                    }
                    Expr::MethodCall(call)
                        if role == "returned value"
                            && call.method == "append_request"
                            && call.args.len() == 2 =>
                    {
                        forwarding_calls += 1;
                        expressions.push((&call.receiver, "receiver"));
                        expressions.push((&call.args[0], "Sanitized input"));
                        expressions.push((&call.args[1], "observation flag"));
                    }
                    Expr::Call(call) if role == "returned value" && call.args.len() == 3 => {
                        let owner_known = matches!(call.func.as_ref(), Expr::Path(value)
                            if value.qself.is_none()
                                && value.path.segments.len() >= 2
                                && value.path.segments.last().is_some_and(|segment| segment.ident == "append_request")
                                && ((value.path.segments.len() == 2
                                    && value.path.segments.first().is_some_and(|segment| segment.ident == "Self"))
                                    || matches!(item_impl.self_ty.as_ref(), Type::Path(owner)
                                        if owner.qself.is_none()
                                            && owner.path.leading_colon.is_some() == value.path.leading_colon.is_some()
                                            && owner.path.segments.len() + 1 == value.path.segments.len()
                                            && owner.path.segments.iter().zip(value.path.segments.iter())
                                                .all(|(left, right)| left.ident == right.ident))));
                        if !owner_known {
                            gap = Some("associated call owner");
                        } else {
                            forwarding_calls += 1;
                            expressions.push((&call.args[0], "receiver"));
                            expressions.push((&call.args[1], "Sanitized input"));
                            expressions.push((&call.args[2], "observation flag"));
                        }
                    }
                    _ => gap = Some(role),
                }
            }
            if let Some(gap) = gap {
                violations.push(format!(
                    "{path}: GlobalLedger::{} forwarding is unresolved at {gap}",
                    method.sig.ident
                ));
            } else if forwarding_calls != 1 || !bindings.is_empty() {
                violations.push(format!(
                    "{path}: GlobalLedger::{} must resolve to the shared append request without unaccounted local bindings",
                    method.sig.ident
                ));
            }
        }
        observation_adapter_valid = observation_methods.len() == 1
            && append_methods.len() == 1
            && request_methods.len() == 1
            && violations.len() == previous_violations;
    }
    for (method, owner) in alternate_ingress_methods {
        if method == "append_with_observation" && observation_adapter_valid {
            continue;
        }
        let path = owner.path.display();
        violations.push(format!(
            "{path}: GlobalLedger exposes alternate public event ingress {method}"
        ));
    }
    Ok(violations)
}

/// Rejects public construction or deserialization of the ledger-owned persisted fact.
pub fn inspect_persisted_event_ownership(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut violations = Vec::new();
    let items = ledger_owners::production_items(&file.items)?;
    let found = inspect_persisted_items(path, &items, &local_type_aliases(&items), &mut violations);
    if found != 1 {
        violations.push(format!(
            "{path}: expected one PersistedEvent definition, found {found}"
        ));
    }
    Ok(violations)
}

fn inspect_persisted_items(
    path: &str,
    items: &[Item],
    aliases: &LocalTypeAliases,
    violations: &mut Vec<String>,
) -> usize {
    let mut found = 0;
    for item in items {
        match item {
            Item::Struct(item_struct) if item_struct.ident == "PersistedEvent" => {
                found += 1;
                if derives_ident(&item_struct.attrs, "Deserialize") {
                    violations.push(format!("{path}: PersistedEvent derives Deserialize"));
                }
                for field in &item_struct.fields {
                    if is_public(&field.vis) {
                        violations.push(format!("{path}: PersistedEvent has a public field"));
                    }
                }
            }
            Item::Impl(item_impl)
                if impl_self_ident(item_impl).is_some_and(|ident| {
                    resolve_alias(&ident.to_string(), aliases) == "PersistedEvent"
                }) =>
            {
                if item_impl
                    .trait_
                    .as_ref()
                    .and_then(|(_, path, _)| path.segments.last())
                    .is_some_and(|segment| {
                        resolve_alias(&segment.ident.to_string(), aliases) == "Deserialize"
                    })
                {
                    violations.push(format!("{path}: PersistedEvent implements Deserialize"));
                }
                for impl_item in &item_impl.items {
                    let syn::ImplItem::Fn(method) = impl_item else {
                        continue;
                    };
                    let has_receiver = method
                        .sig
                        .inputs
                        .iter()
                        .any(|input| matches!(input, FnArg::Receiver(_)));
                    if is_public(&method.vis)
                        && !has_receiver
                        && ["Self", "PersistedEvent"].iter().any(|name| {
                            signature_returns_resolved_ident(&method.sig, name, aliases)
                        })
                    {
                        violations.push(format!(
                            "{path}: PersistedEvent has public constructor {}",
                            method.sig.ident
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    found
}

/// Applies formal fact/event public-surface rules to all discovered production owners.
pub fn inspect_ledger_public_api(owners: &[LedgerOwnerModule]) -> Result<Vec<String>, String> {
    if owners.is_empty() {
        return Err("empty Ledger owner collection".to_string());
    }
    let mut violations = Vec::new();
    let mut facts = 0;
    for owner in owners {
        let path = owner.path.display().to_string();
        facts += inspect_persisted_items(&path, &owner.items, &owner.aliases, &mut violations);
        let items = owner
            .items
            .iter()
            .filter(|item| owner.module != "crate" || formal_ledger_root_item(item, &owner.aliases))
            .cloned()
            .collect::<Vec<_>>();
        inspect_public_items_scoped(
            &path,
            &items,
            Some(&owner.module),
            Some(&owner.aliases),
            &mut violations,
        );
    }
    if facts != 1 {
        violations.push(format!(
            "Ledger owners must define exactly one PersistedEvent, found {facts}"
        ));
    }
    Ok(violations)
}

fn formal_ledger_root_item(item: &Item, aliases: &LocalTypeAliases) -> bool {
    const FORMAL: &[&str] = &[
        "GlobalLedger",
        "PersistedEvent",
        "SanitizedEventDraft",
        "EventDraft",
        "EventPayload",
        "ProjectedEvent",
    ];
    let signature = |sig: &syn::Signature| {
        FORMAL
            .iter()
            .any(|name| signature_returns_resolved_ident(sig, name, aliases))
            || sig.inputs.iter().any(|input| match input {
                FnArg::Typed(input) => FORMAL
                    .iter()
                    .any(|name| type_uses_resolved_ident(&input.ty, name, aliases)),
                FnArg::Receiver(_) => false,
            })
    };
    match item {
        Item::Use(_) => true,
        Item::Impl(item) => {
            impl_self_ident(item).is_some_and(|name| {
                FORMAL.contains(&resolve_alias(&name.to_string(), aliases).as_str())
            }) || item
                .items
                .iter()
                .any(|member| matches!(member, syn::ImplItem::Fn(method) if signature(&method.sig)))
        }
        Item::Fn(item) => signature(&item.sig),
        Item::Struct(item) => {
            FORMAL.contains(&item.ident.to_string().as_str())
                || item.fields.iter().any(|field| {
                    FORMAL
                        .iter()
                        .any(|name| type_uses_resolved_ident(&field.ty, name, aliases))
                })
        }
        Item::Type(item) => FORMAL
            .iter()
            .any(|name| type_uses_resolved_ident(&item.ty, name, aliases)),
        Item::Enum(item) => item
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .any(|field| {
                FORMAL
                    .iter()
                    .any(|name| type_uses_resolved_ident(&field.ty, name, aliases))
            }),
        Item::Trait(item) => item
            .items
            .iter()
            .any(|member| matches!(member, syn::TraitItem::Fn(method) if signature(&method.sig))),
        _ => false,
    }
}

/// Keeps the existing C1 forbidden constructs out of each production owner scope.
pub fn inspect_ledger_forbidden_sources(
    owners: &[LedgerOwnerModule],
) -> Result<Vec<String>, String> {
    if owners.is_empty() {
        return Err("empty Ledger owner collection".to_string());
    }
    let mut violations = Vec::new();
    for owner in owners {
        let mut visitor = C1SourceVisitor {
            path: owner.path.display().to_string(),
            violations: &mut violations,
        };
        for item in &owner.items {
            visitor.visit_item(item);
        }
    }
    Ok(violations)
}

struct C1SourceVisitor<'a> {
    path: String,
    violations: &'a mut Vec<String>,
}
impl C1SourceVisitor<'_> {
    fn production(&mut self, attributes: &[syn::Attribute]) -> bool {
        match ledger_owners::production_attributes(attributes) {
            Ok(production) => production,
            Err(error) => {
                self.violations
                    .push(format!("{}: unresolved C1 cfg scope: {error}", self.path));
                false
            }
        }
    }
    fn inspect(&mut self, value: &str) {
        for forbidden in [
            "ClassifiedField",
            "StructuredPayloadDraft",
            "ErasedSanitizedEventDraft",
            "take_hook",
            "set_hook",
            "catch_unwind",
            "events_after(",
        ] {
            if value.contains(forbidden) {
                self.violations
                    .push(format!("{}: forbidden source token {forbidden}", self.path));
            }
        }
    }
}
impl<'ast> Visit<'ast> for C1SourceVisitor<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        match ledger_owners::item_attributes(item) {
            Ok(attributes) if self.production(attributes) => syn::visit::visit_item(self, item),
            Ok(_) => {}
            Err(error) => self.violations.push(format!("{}: {error}", self.path)),
        }
    }
    fn visit_local(&mut self, local: &'ast syn::Local) {
        if self.production(&local.attrs) {
            syn::visit::visit_local(self, local);
        }
    }
    fn visit_stmt_macro(&mut self, statement: &'ast syn::StmtMacro) {
        if self.production(&statement.attrs) {
            syn::visit::visit_stmt_macro(self, statement);
        }
    }
    fn visit_expr_block(&mut self, expression: &'ast syn::ExprBlock) {
        if self.production(&expression.attrs) {
            syn::visit::visit_expr_block(self, expression);
        }
    }
    fn visit_field_value(&mut self, field: &'ast syn::FieldValue) {
        if self.production(&field.attrs) {
            syn::visit::visit_field_value(self, field);
        }
    }
    fn visit_impl_item_fn(&mut self, method: &'ast syn::ImplItemFn) {
        if self.production(&method.attrs) {
            syn::visit::visit_impl_item_fn(self, method);
        }
    }
    fn visit_trait_item_fn(&mut self, method: &'ast syn::TraitItemFn) {
        if self.production(&method.attrs) {
            syn::visit::visit_trait_item_fn(self, method);
        }
    }
    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        self.inspect(&ident.to_string());
        if ident == "events_after" {
            self.inspect("events_after(");
        }
    }
    fn visit_lit_str(&mut self, value: &'ast syn::LitStr) {
        self.inspect(&value.value());
    }
    fn visit_macro(&mut self, value: &'ast syn::Macro) {
        self.visit_path(&value.path);
        self.inspect(&value.tokens.to_string().replace(' ', ""));
    }
}

/// Rejects any contract reference to the ledger-owned fact or a contract-owned matches method.
pub fn inspect_contract_fact_matching(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let needles = ["PersistedEvent"];
    let mut visitor = IdentTypeVisitor::new(&needles);
    visitor.visit_file(&file);
    let mut violations = Vec::new();
    if visitor.found {
        violations.push(format!(
            "{path}: contract source references ledger-owned PersistedEvent"
        ));
    }
    for item in &file.items {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        if impl_self_ident(item_impl).is_none_or(|ident| ident != "EventQuery") {
            continue;
        }
        if item_impl
            .items
            .iter()
            .any(|item| matches!(item, syn::ImplItem::Fn(method) if method.sig.ident == "matches"))
        {
            violations.push(format!("{path}: EventQuery owns fact matching"));
        }
    }
    Ok(violations)
}

/// Confirms a ledger module contains matching over both EventQuery and PersistedEvent.
pub fn ledger_owns_query_matching(path: &str, source: &str) -> Result<bool, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    Ok(file.items.iter().any(|item| match item {
        Item::Fn(function) => signature_uses_idents(
            &function.sig,
            &["EventQuery", "PersistedEvent"],
        ),
        Item::Impl(item_impl) => item_impl.items.iter().any(|item| {
            matches!(item, syn::ImplItem::Fn(method) if signature_uses_idents(&method.sig, &["EventQuery", "PersistedEvent"]))
        }),
        _ => false,
    }))
}

/// Workflow #314 FENCED-CLOSE: inspect the issuing bridge and its close consumers.
/// Import aliases are propagated across this bounded workspace inventory, including
/// re-exports. This is a source guard, not a claim of Rust cross-crate privacy.
pub fn inspect_fenced_close_sources(sources: &[(String, String)]) -> Result<Vec<String>, String> {
    let mut parsed = Vec::new();
    for (path, source) in sources {
        let file = syn::parse_file(source).map_err(|error| format!("parse {path}: {error}"))?;
        parsed.push((path, ledger_owners::production_items(&file.items)?));
    }
    let mut issuer_names = HashSet::from(["issue_fenced_write".to_string()]);
    let mut witness_names = HashSet::from(["FencedWrite".to_string()]);
    struct Imports(Vec<(String, String)>);
    impl<'ast> Visit<'ast> for Imports {
        fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
            self.0.extend(public_use_aliases(&item.tree));
        }
    }
    let mut imports = Imports(Vec::new());
    for (_, items) in &parsed {
        for item in items {
            imports.visit_item(item);
        }
    }
    loop {
        let before = (issuer_names.len(), witness_names.len());
        for (local, target) in &imports.0 {
            if issuer_names.contains(target) {
                issuer_names.insert(local.clone());
            }
            if witness_names.contains(target) {
                witness_names.insert(local.clone());
            }
        }
        for (_, items) in &parsed {
            let aliases = local_type_aliases(items);
            for (local, target) in &aliases.names {
                if issuer_names.contains(target) {
                    issuer_names.insert(local.clone());
                }
                if witness_names.contains(target) {
                    witness_names.insert(local.clone());
                }
            }
        }
        if before == (issuer_names.len(), witness_names.len()) {
            break;
        }
    }
    let mut violations = Vec::new();
    let mut issuers = HashSet::new();
    let mut witness_definitions = 0;
    let mut close_authorities = 0;
    let mut bridge_definitions = 0;
    for (path, items) in &parsed {
        let aliases = local_type_aliases(items);
        let mut visitor = FencedCloseVisitor {
            path,
            aliases: &aliases,
            issuer_names: &issuer_names,
            witness_names: &witness_names,
            function: String::new(),
            self_type: String::new(),
            violations: &mut violations,
            issuers: &mut issuers,
        };
        for item in items {
            visitor.visit_item(item);
        }
        let mut nested = Vec::new();
        collect_nested_items(items, &mut nested);
        for item in nested {
            match item {
                Item::Struct(value) if value.ident == "FencedWrite" => {
                    witness_definitions += 1;
                    if value
                        .fields
                        .iter()
                        .any(|field| !matches!(field.vis, Visibility::Inherited))
                        || ["Copy", "Clone", "Default", "Serialize", "Deserialize"]
                            .iter()
                            .any(|name| derives_ident(&value.attrs, name))
                    {
                        violations.push(format!(
                            "{path}: FencedWrite must remain opaque and non-copyable"
                        ));
                    }
                }
                Item::Fn(value) if value.sig.ident == "issue_fenced_write" => {
                    bridge_definitions += 1;
                    if path.as_str() != "crates/actingcommand-contract/src/runtime.rs" {
                        violations.push(format!("{path}: unexpected witness issuing bridge"));
                    }
                }
                Item::Impl(value)
                    if impl_self_ident(value)
                        .is_some_and(|name| witness_names.contains(&name.to_string())) =>
                {
                    if let Some((_, target, _)) = &value.trait_
                        && target.segments.last().is_some_and(|name| {
                            [
                                "Copy",
                                "Clone",
                                "Default",
                                "Serialize",
                                "Deserialize",
                                "From",
                                "TryFrom",
                            ]
                            .contains(&name.ident.to_string().as_str())
                        })
                    {
                        violations.push(format!(
                            "{path}: witness trait opens copying or construction"
                        ));
                    }
                    for method in &value.items {
                        if let syn::ImplItem::Fn(method) = method
                            && is_public(&method.vis)
                            && !method
                                .sig
                                .inputs
                                .iter()
                                .any(|input| matches!(input, FnArg::Receiver(_)))
                            && signature_returns_any_resolved_ident(
                                &method.sig,
                                &["Self".into(), "FencedWrite".into()],
                                &aliases,
                            )
                        {
                            violations.push(format!(
                                "{path}: witness exposes constructor {}",
                                method.sig.ident
                            ));
                        }
                    }
                }
                Item::Enum(value) if value.ident == "DeviceCloseAuthority" => {
                    close_authorities += 1;
                    if !value.variants.iter().any(|variant| {
                        variant.ident == "FencedDeviceWrite"
                            && variant.fields.len() == 1
                            && variant.fields.iter().any(|field| {
                                type_uses_resolved_ident(&field.ty, "FencedWrite", &aliases)
                            })
                    }) {
                        violations.push(format!(
                            "{path}: device-effect close must carry FencedWrite"
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    if witness_definitions != 1 || close_authorities != 1 || bridge_definitions != 1 {
        violations.push(
            "fenced close requires one witness, issuing bridge and close authority".to_string(),
        );
    }
    if issuers
        != HashSet::from([
            "begin_destructive_step".to_string(),
            "begin_resource_close".to_string(),
        ])
    {
        violations.push(format!(
            "fenced close production issuers differ from scheduler admission: {issuers:?}"
        ));
    }
    Ok(violations)
}

struct FencedCloseVisitor<'a> {
    path: &'a str,
    aliases: &'a LocalTypeAliases,
    issuer_names: &'a HashSet<String>,
    witness_names: &'a HashSet<String>,
    function: String,
    self_type: String,
    violations: &'a mut Vec<String>,
    issuers: &'a mut HashSet<String>,
}

impl FencedCloseVisitor<'_> {
    fn close_signature(&mut self, signature: &syn::Signature) {
        let name = signature.ident.to_string();
        let close = name == "close_once"
            || (self.path == "crates/device/src/capture.rs"
                && ["NemuIpcWorker", "NemuIpcWorkerState"].contains(&self.self_type.as_str())
                && [
                    "shutdown_once",
                    "shutdown_with_input_check",
                    "disconnect",
                    "close",
                ]
                .contains(&name.as_str()))
            || (self.path == "crates/device/src/capture/nemu_input.rs"
                && name == "close_input_contact");
        if close && !signature.inputs.iter().any(|argument| matches!(argument,
            FnArg::Typed(argument) if type_uses_resolved_ident(&argument.ty, "DeviceCloseAuthority", self.aliases))) {
            self.violations.push(format!(
                "{}: {}::{name} lost close authority",
                self.path, self.self_type
            ));
        }
    }
}

impl<'ast> Visit<'ast> for FencedCloseVisitor<'_> {
    fn visit_item_fn(&mut self, function: &'ast ItemFn) {
        if has_cfg_test(&function.attrs)
            || function
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("test"))
        {
            return;
        }
        let prior = std::mem::replace(&mut self.function, function.sig.ident.to_string());
        syn::visit::visit_item_fn(self, function);
        self.function = prior;
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let prior = std::mem::replace(
            &mut self.self_type,
            impl_self_ident(item)
                .map(ToString::to_string)
                .unwrap_or_default(),
        );
        syn::visit::visit_item_impl(self, item);
        self.self_type = prior;
    }

    fn visit_impl_item_fn(&mut self, method: &'ast syn::ImplItemFn) {
        let prior = std::mem::replace(&mut self.function, method.sig.ident.to_string());
        self.close_signature(&method.sig);
        syn::visit::visit_impl_item_fn(self, method);
        self.function = prior;
    }

    fn visit_trait_item_fn(&mut self, method: &'ast syn::TraitItemFn) {
        self.close_signature(&method.sig);
        syn::visit::visit_trait_item_fn(self, method);
    }

    fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
        if expression
            .path
            .segments
            .last()
            .is_some_and(|name| self.issuer_names.contains(&name.ident.to_string()))
        {
            if self.path == "crates/scheduler/src/lib.rs"
                && self.self_type == "SeedScheduler"
                && ["begin_destructive_step", "begin_resource_close"]
                    .contains(&self.function.as_str())
            {
                self.issuers.insert(self.function.clone());
            } else {
                self.violations.push(format!(
                    "{}: {} references the scheduler issuing bridge",
                    self.path, self.function
                ));
            }
        }
        syn::visit::visit_expr_path(self, expression);
    }

    fn visit_expr_struct(&mut self, expression: &'ast syn::ExprStruct) {
        if expression
            .path
            .segments
            .last()
            .is_some_and(|name| self.witness_names.contains(&name.ident.to_string()))
            && !(self.path == "crates/actingcommand-contract/src/runtime.rs"
                && self.function == "issue_fenced_write")
        {
            self.violations.push(format!(
                "{}: {} constructs a witness outside its bridge",
                self.path, self.function
            ));
        }
        syn::visit::visit_expr_struct(self, expression);
    }
}

/// Enforces issuer-only producer IDs and store-issued artifact attachments.
pub fn inspect_producer_event_capabilities(
    path: &str,
    source: &str,
) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let aliases = local_type_aliases(&file.items);
    let mut items = Vec::new();
    collect_nested_items(&file.items, &mut items);
    let mut violations = Vec::new();
    let mut found_store_issued_artifact = false;
    for item in items.iter().copied() {
        inspect_public_artifact_capability_routes(path, item, &aliases, &mut violations);
        match item {
            Item::Impl(item_impl)
                if impl_self_ident(item_impl).is_some_and(|ident| ident == "EventDraft") =>
            {
                for impl_item in &item_impl.items {
                    let syn::ImplItem::Fn(method) = impl_item else {
                        continue;
                    };
                    if !is_public(&method.vis) {
                        continue;
                    }
                    if method.sig.ident == "new"
                        && (method_argument_type(method, "event_id")
                            .and_then(|ty| resolved_type_ident(ty, &aliases))
                            .is_none_or(|ident| ident != "IssuedEventId")
                            || method_argument_type(method, "links")
                                .and_then(|ty| resolved_type_ident(ty, &aliases))
                                .is_none_or(|ident| ident != "EventLinksDraft"))
                    {
                        violations.push(format!(
                            "{path}: EventDraft::new must require IssuedEventId and EventLinksDraft"
                        ));
                    }
                    if method.sig.ident == "with_artifacts"
                        && !method_argument_type(method, "artifacts").is_some_and(|ty| {
                            vec_inner_resolved_ident(ty, &aliases)
                                .is_some_and(|ident| ident == "StoreIssuedArtifact")
                        })
                    {
                        violations.push(format!(
                            "{path}: EventDraft::with_artifacts must require StoreIssuedArtifact"
                        ));
                    }
                }
            }
            Item::Impl(item_impl)
                if impl_self_ident(item_impl).is_some_and(|ident| ident == "EventLinksDraft") =>
            {
                let expected = [
                    ("with_instance_id", "IssuedInstanceId"),
                    ("with_request_id", "IssuedRequestId"),
                    ("with_correlation_id", "IssuedCorrelationId"),
                    ("with_causation_id", "IssuedCausationId"),
                    ("with_task_id", "IssuedTaskId"),
                    ("with_run_id", "IssuedRunId"),
                    ("with_lease_id", "IssuedLeaseId"),
                    ("with_frame_id", "IssuedFrameId"),
                    ("with_action_id", "IssuedActionId"),
                    ("with_recognition_id", "IssuedRecognitionId"),
                ];
                for (name, expected_type) in expected {
                    let method = item_impl.items.iter().find_map(|item| match item {
                        syn::ImplItem::Fn(method) if method.sig.ident == name => Some(method),
                        _ => None,
                    });
                    let valid = method.is_some_and(|method| {
                        is_public(&method.vis)
                            && method_argument_type(method, "value")
                                .and_then(|ty| resolved_type_ident(ty, &aliases))
                                .is_some_and(|ident| ident == expected_type)
                    });
                    if !valid {
                        violations.push(format!(
                            "{path}: EventLinksDraft::{name} must require {expected_type}"
                        ));
                    }
                }
            }
            Item::Struct(item_struct) if item_struct.ident == "StoreIssuedArtifact" => {
                found_store_issued_artifact = true;
                if derives_ident(&item_struct.attrs, "Serialize")
                    || derives_ident(&item_struct.attrs, "Deserialize")
                {
                    violations.push(format!(
                        "{path}: StoreIssuedArtifact must not be serializable or deserializable"
                    ));
                }
                if item_struct.fields.iter().any(|field| is_public(&field.vis)) {
                    violations.push(format!(
                        "{path}: StoreIssuedArtifact must not expose public fields"
                    ));
                }
            }
            Item::Struct(item_struct) if item_struct.ident == "ArtifactStoreIssuer" => {
                if !is_public(&item_struct.vis)
                    || derives_ident(&item_struct.attrs, "Serialize")
                    || derives_ident(&item_struct.attrs, "Deserialize")
                    || item_struct.fields.iter().any(|field| is_public(&field.vis))
                {
                    violations.push(format!(
                        "{path}: ArtifactStoreIssuer must be public, opaque, and non-serializable"
                    ));
                }
            }
            Item::Type(item_type) if is_public(&item_type.vis) => {
                if item_type.ident == "ArtifactStoreIssuer" {
                    violations.push(format!(
                        "{path}: public ArtifactStoreIssuer alias is forbidden until the real store boundary exists"
                    ));
                }
                if type_uses_resolved_ident(&item_type.ty, "StoreIssuedArtifact", &aliases) {
                    violations.push(format!(
                        "{path}: public type alias {} exposes StoreIssuedArtifact",
                        item_type.ident
                    ));
                }
            }
            Item::Use(item_use) if is_public(&item_use.vis) => {
                for (local, target) in public_use_aliases(&item_use.tree) {
                    if local == "StoreIssuedArtifact" || target == "StoreIssuedArtifact" {
                        violations.push(format!(
                            "{path}: public use exposes StoreIssuedArtifact under {local}"
                        ));
                    }
                    if local == "StaticCode" || target == "StaticCode" {
                        violations
                            .push(format!("{path}: producer event surface retains StaticCode"));
                    }
                }
            }
            Item::Impl(item_impl)
                if impl_self_ident(item_impl).is_some_and(|ident| ident == "ArtifactReference") =>
            {
                for impl_item in &item_impl.items {
                    let syn::ImplItem::Fn(method) = impl_item else {
                        continue;
                    };
                    let has_receiver = method
                        .sig
                        .inputs
                        .iter()
                        .any(|input| matches!(input, FnArg::Receiver(_)));
                    if is_public(&method.vis)
                        && !has_receiver
                        && signature_returns_ident(&method.sig, &["Self", "ArtifactReference"])
                    {
                        violations.push(format!(
                            "{path}: ArtifactReference has public constructor {}",
                            method.sig.ident
                        ));
                    }
                }
            }
            Item::Impl(item_impl)
                if impl_self_ident(item_impl)
                    .is_some_and(|ident| ident == "StoreIssuedArtifact") =>
            {
                if let Some((_, trait_path, _)) = &item_impl.trait_
                    && trait_path.segments.last().is_some_and(|segment| {
                        matches!(
                            segment.ident.to_string().as_str(),
                            "Serialize"
                                | "Deserialize"
                                | "From"
                                | "TryFrom"
                                | "FromStr"
                                | "Default"
                        )
                    })
                {
                    violations.push(format!(
                        "{path}: StoreIssuedArtifact implements producer-visible constructor or serde trait"
                    ));
                }
                for impl_item in &item_impl.items {
                    let syn::ImplItem::Fn(method) = impl_item else {
                        continue;
                    };
                    let has_receiver = method
                        .sig
                        .inputs
                        .iter()
                        .any(|input| matches!(input, FnArg::Receiver(_)));
                    if is_public(&method.vis)
                        && !has_receiver
                        && signature_returns_resolved_ident(
                            &method.sig,
                            "StoreIssuedArtifact",
                            &aliases,
                        )
                    {
                        violations.push(format!(
                            "{path}: StoreIssuedArtifact has public constructor {}",
                            method.sig.ident
                        ));
                    }
                }
            }
            Item::Impl(item_impl) if impl_self_ident(item_impl).is_some_and(is_transport_id) => {
                if item_impl
                    .trait_
                    .as_ref()
                    .and_then(|(_, path, _)| path.segments.last())
                    .is_some_and(|segment| segment.ident == "Display")
                {
                    violations.push(format!(
                        "{path}: transport identifier {} exposes Display",
                        impl_self_ident(item_impl).expect("transport impl")
                    ));
                }
                for impl_item in &item_impl.items {
                    let syn::ImplItem::Fn(method) = impl_item else {
                        continue;
                    };
                    let has_receiver = method
                        .sig
                        .inputs
                        .iter()
                        .any(|input| matches!(input, FnArg::Receiver(_)));
                    if is_public(&method.vis)
                        && !has_receiver
                        && signature_returns_any_resolved_ident(
                            &method.sig,
                            &[
                                "Self".to_string(),
                                impl_self_ident(item_impl)
                                    .expect("transport impl")
                                    .to_string(),
                            ],
                            &aliases,
                        )
                    {
                        violations.push(format!(
                            "{path}: transport identifier {} exposes public constructor {}",
                            impl_self_ident(item_impl).expect("transport impl"),
                            method.sig.ident
                        ));
                    }
                }
            }
            Item::Fn(function) if is_public(&function.vis) => {
                if signature_returns_resolved_ident(&function.sig, "StoreIssuedArtifact", &aliases)
                {
                    violations.push(format!(
                        "{path}: public function {} returns StoreIssuedArtifact",
                        function.sig.ident
                    ));
                }
            }
            Item::Trait(item_trait) if is_public(&item_trait.vis) => {
                for trait_item in &item_trait.items {
                    let syn::TraitItem::Fn(method) = trait_item else {
                        continue;
                    };
                    if signature_returns_resolved_ident(
                        &method.sig,
                        "StoreIssuedArtifact",
                        &aliases,
                    ) {
                        violations.push(format!(
                            "{path}: public trait method {}::{} returns StoreIssuedArtifact",
                            item_trait.ident, method.sig.ident
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if !found_store_issued_artifact {
        violations.push(format!(
            "{path}: missing concrete StoreIssuedArtifact capability definition"
        ));
    }
    if defined_or_aliased_static_code(&items, &aliases) {
        violations.push(format!("{path}: producer event surface retains StaticCode"));
    }
    Ok(violations)
}

fn inspect_public_artifact_capability_routes(
    path: &str,
    item: &Item,
    aliases: &LocalTypeAliases,
    violations: &mut Vec<String>,
) {
    match item {
        Item::Fn(function)
            if is_public(&function.vis)
                && signature_returns_resolved_ident(
                    &function.sig,
                    "StoreIssuedArtifact",
                    aliases,
                ) =>
        {
            violations.push(format!(
                "{path}: public function {} returns StoreIssuedArtifact",
                function.sig.ident
            ));
        }
        Item::Const(item_const)
            if is_public(&item_const.vis)
                && type_uses_resolved_ident(&item_const.ty, "StoreIssuedArtifact", aliases) =>
        {
            violations.push(format!(
                "{path}: public const {} exposes StoreIssuedArtifact",
                item_const.ident
            ));
        }
        Item::Static(item_static)
            if is_public(&item_static.vis)
                && type_uses_resolved_ident(&item_static.ty, "StoreIssuedArtifact", aliases) =>
        {
            violations.push(format!(
                "{path}: public static {} exposes StoreIssuedArtifact",
                item_static.ident
            ));
        }
        Item::Struct(item_struct)
            if is_public(&item_struct.vis) && item_struct.ident != "StoreIssuedArtifact" =>
        {
            for field in &item_struct.fields {
                if is_public(&field.vis)
                    && type_uses_resolved_ident(&field.ty, "StoreIssuedArtifact", aliases)
                {
                    violations.push(format!(
                        "{path}: public struct {} exposes StoreIssuedArtifact",
                        item_struct.ident
                    ));
                }
            }
        }
        Item::Enum(item_enum) if is_public(&item_enum.vis) => {
            for variant in &item_enum.variants {
                if variant.fields.iter().any(|field| {
                    type_uses_resolved_ident(&field.ty, "StoreIssuedArtifact", aliases)
                }) {
                    violations.push(format!(
                        "{path}: public enum {} exposes StoreIssuedArtifact",
                        item_enum.ident
                    ));
                }
            }
        }
        Item::Union(item_union) if is_public(&item_union.vis) => {
            if item_union.fields.named.iter().any(|field| {
                is_public(&field.vis)
                    && type_uses_resolved_ident(&field.ty, "StoreIssuedArtifact", aliases)
            }) {
                violations.push(format!(
                    "{path}: public union {} exposes StoreIssuedArtifact",
                    item_union.ident
                ));
            }
        }
        Item::Impl(item_impl) => {
            let trait_impl = item_impl.trait_.is_some();
            let self_is_capability =
                impl_self_ident(item_impl).is_some_and(|ident| ident == "StoreIssuedArtifact");
            for impl_item in &item_impl.items {
                match impl_item {
                    syn::ImplItem::Fn(method) => {
                        let externally_callable = trait_impl || is_public(&method.vis);
                        let returns_capability = signature_returns_resolved_ident(
                            &method.sig,
                            "StoreIssuedArtifact",
                            aliases,
                        ) || (self_is_capability
                            && signature_returns_ident(&method.sig, &["Self"]));
                        let approved_store_issue = impl_self_ident(item_impl)
                            .is_some_and(|ident| ident == "ArtifactStoreIssuer")
                            && method.sig.ident == "issue"
                            && method_argument_type(method, "kind")
                                .and_then(|ty| resolved_type_ident(ty, aliases))
                                .is_some_and(|ident| ident == "ArtifactKind")
                            && method_argument_type(method, "links")
                                .and_then(|ty| resolved_type_ident(ty, aliases))
                                .is_some_and(|ident| ident == "ArtifactLinksDraft")
                            && method_argument_type(method, "policy")
                                .and_then(|ty| resolved_type_ident(ty, aliases))
                                .is_some_and(|ident| ident == "ArtifactIssuePolicy");
                        if externally_callable && returns_capability && !approved_store_issue {
                            let owner = impl_self_ident(item_impl)
                                .map_or_else(|| "<unknown>".to_string(), ToString::to_string);
                            violations.push(format!(
                                "{path}: externally callable method {owner}::{} returns StoreIssuedArtifact",
                                method.sig.ident
                            ));
                        }
                    }
                    syn::ImplItem::Const(item_const)
                        if (trait_impl || is_public(&item_const.vis))
                            && type_uses_resolved_ident(
                                &item_const.ty,
                                "StoreIssuedArtifact",
                                aliases,
                            ) =>
                    {
                        violations.push(format!(
                            "{path}: externally visible associated const exposes StoreIssuedArtifact"
                        ));
                    }
                    syn::ImplItem::Type(item_type)
                        if trait_impl
                            && type_uses_resolved_ident(
                                &item_type.ty,
                                "StoreIssuedArtifact",
                                aliases,
                            ) =>
                    {
                        violations.push(format!(
                            "{path}: trait implementation exposes StoreIssuedArtifact as an associated type"
                        ));
                    }
                    _ => {}
                }
            }
        }
        Item::Trait(item_trait) if is_public(&item_trait.vis) => {
            for trait_item in &item_trait.items {
                match trait_item {
                    syn::TraitItem::Fn(method)
                        if signature_returns_resolved_ident(
                            &method.sig,
                            "StoreIssuedArtifact",
                            aliases,
                        ) =>
                    {
                        violations.push(format!(
                            "{path}: public trait method {}::{} returns StoreIssuedArtifact",
                            item_trait.ident, method.sig.ident
                        ));
                    }
                    syn::TraitItem::Const(item_const)
                        if type_uses_resolved_ident(
                            &item_const.ty,
                            "StoreIssuedArtifact",
                            aliases,
                        ) =>
                    {
                        violations.push(format!(
                            "{path}: public trait {} exposes StoreIssuedArtifact in an associated const",
                            item_trait.ident
                        ));
                    }
                    syn::TraitItem::Type(item_type)
                        if item_type.default.as_ref().is_some_and(|(_, ty)| {
                            type_uses_resolved_ident(ty, "StoreIssuedArtifact", aliases)
                        }) =>
                    {
                        violations.push(format!(
                            "{path}: public trait {} exposes StoreIssuedArtifact as an associated type",
                            item_trait.ident
                        ));
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn collect_nested_items<'a>(items: &'a [Item], collected: &mut Vec<&'a Item>) {
    for item in items {
        collected.push(item);
        if let Item::Mod(item_mod) = item
            && let Some((_, nested)) = &item_mod.content
        {
            collect_nested_items(nested, collected);
        }
    }
}

fn impl_self_ident(item_impl: &syn::ItemImpl) -> Option<&syn::Ident> {
    let Type::Path(path) = item_impl.self_ty.as_ref() else {
        return None;
    };
    path.path.segments.last().map(|segment| &segment.ident)
}

fn pattern_ident(pattern: &Pat) -> Option<&syn::Ident> {
    let Pat::Ident(ident) = pattern else {
        return None;
    };
    Some(&ident.ident)
}

fn type_last_ident(value_type: &Type) -> Option<&syn::Ident> {
    let Type::Path(path) = value_type else {
        return None;
    };
    path.path.segments.last().map(|segment| &segment.ident)
}

#[derive(Default)]
struct LocalTypeAliases {
    names: HashMap<String, String>,
}

fn local_type_aliases(items: &[Item]) -> LocalTypeAliases {
    let mut aliases = LocalTypeAliases::default();
    collect_local_type_aliases(items, &mut aliases);
    while promote_store_capability_aliases(items, &mut aliases) {}
    aliases
}

fn promote_store_capability_aliases(items: &[Item], aliases: &mut LocalTypeAliases) -> bool {
    let mut changed = false;
    for item in items {
        match item {
            Item::Type(item_type)
                if type_uses_resolved_ident(&item_type.ty, "StoreIssuedArtifact", aliases)
                    && resolve_alias(&item_type.ident.to_string(), aliases)
                        != "StoreIssuedArtifact" =>
            {
                aliases.names.insert(
                    item_type.ident.to_string(),
                    "StoreIssuedArtifact".to_string(),
                );
                changed = true;
            }
            Item::Mod(item_mod) => {
                if let Some((_, nested)) = &item_mod.content {
                    changed |= promote_store_capability_aliases(nested, aliases);
                }
            }
            _ => {}
        }
    }
    changed
}

fn collect_local_type_aliases(items: &[Item], aliases: &mut LocalTypeAliases) {
    for item in items {
        match item {
            Item::Use(item_use) => collect_type_alias(&mut Vec::new(), &item_use.tree, aliases),
            Item::Type(item_type) => {
                if let Some(target) = type_last_ident(&item_type.ty) {
                    aliases
                        .names
                        .insert(item_type.ident.to_string(), target.to_string());
                }
            }
            Item::Mod(item_mod) => {
                if let Some((_, nested)) = &item_mod.content {
                    collect_local_type_aliases(nested, aliases);
                }
            }
            _ => {}
        }
    }
}

fn collect_type_alias(prefix: &mut Vec<String>, tree: &UseTree, aliases: &mut LocalTypeAliases) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_type_alias(prefix, &path.tree, aliases);
            prefix.pop();
        }
        UseTree::Name(name) => {
            aliases
                .names
                .insert(name.ident.to_string(), name.ident.to_string());
        }
        UseTree::Rename(rename) => {
            let target = if rename.ident == "self" {
                prefix
                    .last()
                    .cloned()
                    .unwrap_or_else(|| rename.ident.to_string())
            } else {
                rename.ident.to_string()
            };
            aliases.names.insert(rename.rename.to_string(), target);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_type_alias(prefix, item, aliases);
            }
        }
        _ => {}
    }
}

fn resolve_alias(name: &str, aliases: &LocalTypeAliases) -> String {
    let mut current = name.to_string();
    let mut visited = HashSet::new();
    while visited.insert(current.clone()) {
        let Some(next) = aliases.names.get(&current) else {
            break;
        };
        if next == &current {
            break;
        }
        current = next.clone();
    }
    current
}

fn resolved_type_ident(value_type: &Type, aliases: &LocalTypeAliases) -> Option<String> {
    type_last_ident(value_type).map(|ident| resolve_alias(&ident.to_string(), aliases))
}

fn vec_inner_resolved_ident(value_type: &Type, aliases: &LocalTypeAliases) -> Option<String> {
    let Type::Path(path) = value_type else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Vec" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(inner) => resolved_type_ident(inner, aliases),
        _ => None,
    })
}

fn signature_returns_resolved_ident(
    signature: &syn::Signature,
    needle: &str,
    aliases: &LocalTypeAliases,
) -> bool {
    let ReturnType::Type(_, output) = &signature.output else {
        return false;
    };
    type_uses_resolved_ident(output, needle, aliases)
}

fn signature_returns_any_resolved_ident(
    signature: &syn::Signature,
    needles: &[String],
    aliases: &LocalTypeAliases,
) -> bool {
    let ReturnType::Type(_, output) = &signature.output else {
        return false;
    };
    needles
        .iter()
        .any(|needle| type_uses_resolved_ident(output, needle, aliases))
}

fn type_uses_resolved_ident(value_type: &Type, needle: &str, aliases: &LocalTypeAliases) -> bool {
    let mut visitor = ResolvedIdentVisitor {
        aliases,
        needle,
        found: false,
    };
    visitor.visit_type(value_type);
    visitor.found
}

struct ResolvedIdentVisitor<'a> {
    aliases: &'a LocalTypeAliases,
    needle: &'a str,
    found: bool,
}

impl<'ast> Visit<'ast> for ResolvedIdentVisitor<'_> {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        if let Some(segment) = node.path.segments.last()
            && resolve_alias(&segment.ident.to_string(), self.aliases) == self.needle
        {
            self.found = true;
        }
        syn::visit::visit_type_path(self, node);
    }
}

fn public_use_aliases(tree: &UseTree) -> Vec<(String, String)> {
    let mut aliases = Vec::new();
    collect_public_use_alias(&mut Vec::new(), tree, &mut aliases);
    aliases
}

fn collect_public_use_alias(
    prefix: &mut Vec<String>,
    tree: &UseTree,
    aliases: &mut Vec<(String, String)>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_public_use_alias(prefix, &path.tree, aliases);
            prefix.pop();
        }
        UseTree::Name(name) => {
            aliases.push((name.ident.to_string(), name.ident.to_string()));
        }
        UseTree::Rename(rename) => {
            let target = if rename.ident == "self" {
                prefix
                    .last()
                    .cloned()
                    .unwrap_or_else(|| rename.ident.to_string())
            } else {
                rename.ident.to_string()
            };
            aliases.push((rename.rename.to_string(), target));
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_public_use_alias(prefix, item, aliases);
            }
        }
        _ => {}
    }
}

fn is_transport_id(candidate: &syn::Ident) -> bool {
    matches!(
        candidate.to_string().as_str(),
        "EventId"
            | "InstanceId"
            | "RequestId"
            | "CorrelationId"
            | "CausationId"
            | "TaskId"
            | "RunId"
            | "LeaseId"
            | "FrameId"
            | "ActionId"
            | "RecognitionId"
            | "ArtifactId"
    )
}

fn defined_or_aliased_static_code(items: &[&Item], aliases: &LocalTypeAliases) -> bool {
    items.iter().any(|item| match item {
        Item::Struct(item_struct) => item_struct.ident == "StaticCode",
        Item::Enum(item_enum) => item_enum.ident == "StaticCode",
        Item::Type(item_type) => {
            item_type.ident == "StaticCode"
                || resolved_type_ident(&item_type.ty, aliases)
                    .is_some_and(|ident| ident == "StaticCode")
        }
        Item::Use(item_use) => public_use_aliases(&item_use.tree)
            .into_iter()
            .any(|(local, target)| local == "StaticCode" || target == "StaticCode"),
        _ => false,
    })
}

fn method_argument_type<'a>(method: &'a syn::ImplItemFn, name: &str) -> Option<&'a Type> {
    method.sig.inputs.iter().find_map(|input| {
        let FnArg::Typed(argument) = input else {
            return None;
        };
        pattern_ident(&argument.pat)
            .is_some_and(|ident| ident == name)
            .then_some(argument.ty.as_ref())
    })
}

fn method_accepts_event_ingress(method: &syn::ImplItemFn) -> bool {
    method.sig.inputs.iter().any(|input| {
        let FnArg::Typed(argument) = input else {
            return false;
        };
        [
            "EventDraft",
            "SanitizedEventDraft",
            "EventPayloadDraft",
            "ArtifactReference",
            "PersistedEvent",
        ]
        .into_iter()
        .any(|needle| type_contains_ident(&argument.ty, needle))
    })
}

fn derives_ident(attributes: &[syn::Attribute], needle: &str) -> bool {
    attributes.iter().any(|attribute| {
        if !attribute.path().is_ident("derive") {
            return false;
        }
        attribute
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
            .is_ok_and(|paths| {
                paths.iter().any(|path| {
                    path.segments
                        .last()
                        .is_some_and(|segment| segment.ident == needle)
                })
            })
    })
}

fn signature_returns_ident(signature: &syn::Signature, needles: &[&str]) -> bool {
    let ReturnType::Type(_, output) = &signature.output else {
        return false;
    };
    needles
        .iter()
        .any(|needle| type_contains_ident(output, needle))
}

fn signature_uses_idents(signature: &syn::Signature, needles: &[&str]) -> bool {
    needles.iter().all(|needle| {
        let in_inputs = signature.inputs.iter().any(|input| match input {
            FnArg::Receiver(_) => false,
            FnArg::Typed(argument) => type_contains_ident(&argument.ty, needle),
        });
        let in_output = match &signature.output {
            ReturnType::Default => false,
            ReturnType::Type(_, output) => type_contains_ident(output, needle),
        };
        in_inputs || in_output
    })
}

fn type_contains_ident(value_type: &Type, needle: &str) -> bool {
    let needles = [needle];
    let mut visitor = IdentTypeVisitor::new(&needles);
    visitor.visit_type(value_type);
    visitor.found
}

struct IdentTypeVisitor<'a> {
    needles: &'a [&'a str],
    found: bool,
}

impl<'a> IdentTypeVisitor<'a> {
    fn new(needles: &'a [&'a str]) -> Self {
        Self {
            needles,
            found: false,
        }
    }
}

impl<'ast> Visit<'ast> for IdentTypeVisitor<'_> {
    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        if node
            .path
            .segments
            .iter()
            .any(|segment| self.needles.iter().any(|needle| segment.ident == *needle))
        {
            self.found = true;
        }
        syn::visit::visit_type_path(self, node);
    }
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
        "sha2".to_string(),
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
    dependency_boundary_violations(metadata, "actingcommand-lab", optional_packages)
}

/// Finds direct or transitive dependency paths from production packages to developer-only tooling.
pub fn resource_tooling_removability_violations(
    metadata: &str,
    optional_packages: &[&str],
) -> Result<Vec<String>, String> {
    dependency_boundary_violations(
        metadata,
        "actingcommand-resource-tooling",
        optional_packages,
    )
}

fn dependency_boundary_violations(
    metadata: &str,
    target_package: &str,
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
    let mut target_ids = Vec::new();
    for package in packages {
        let id = required_field_string(package, "id")?;
        let name = required_field_string(package, "name")?;
        if name == target_package {
            target_ids.push(id.clone());
        }
        package_names.insert(id, name);
    }
    if target_ids.len() > 1 {
        return Err(format!(
            "cargo metadata contains multiple {target_package} packages"
        ));
    }
    let Some(target_id) = target_ids.pop() else {
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
        let Some(path) = dependency_path(&root, &target_id, &dependencies) else {
            continue;
        };
        let names = path
            .iter()
            .map(|id| package_names.get(id).cloned().unwrap_or_else(|| id.clone()))
            .collect::<Vec<_>>();
        violations.push(format!(
            "production package {root_name} reaches {target_package}: {}",
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
    inspect_public_items_scoped(path, items, module, None, violations);
}

fn inspect_public_items_scoped(
    path: &str,
    items: &[Item],
    module: Option<&str>,
    scope: Option<&LocalTypeAliases>,
    violations: &mut Vec<String>,
) {
    let mut aliases = serde_json_value_aliases(items);
    let mut ledger_aliases = ledger_storage_aliases(items);
    if let Some(scope) = scope {
        for name in scope.names.keys() {
            let target = resolve_alias(name, scope);
            if target == "Value" {
                aliases.values.insert(name.clone());
            }
            if target == "serde_json" {
                aliases.modules.insert(name.clone());
            }
            if is_ledger_storage_type(&target) {
                ledger_aliases.types.insert(name.clone());
            }
            if target == "actingcommand_ledger" {
                ledger_aliases.modules.insert(name.clone());
            }
        }
    }
    loop {
        let mut changed = false;
        for item in items {
            if let Item::Type(item) = item {
                if type_uses_json_value(&item.ty, &aliases) {
                    changed |= aliases.values.insert(item.ident.to_string());
                }
                if type_uses_ledger_storage(&item.ty, &ledger_aliases) {
                    changed |= ledger_aliases.types.insert(item.ident.to_string());
                }
            }
        }
        if !changed {
            break;
        }
    }
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
                    if !is_public(&field.vis) {
                        continue;
                    }
                    let field_name = field
                        .ident
                        .as_ref()
                        .map_or_else(|| index.to_string(), ToString::to_string);
                    let name = format!("{}::{field_name}", item_struct.ident);
                    if type_uses_json_value(&field.ty, &aliases) {
                        violations.push(format!(
                            "{path}: public field {} uses serde_json::Value",
                            qualified(module, &name)
                        ));
                    }
                    if type_uses_ledger_storage(&field.ty, &ledger_aliases) {
                        violations.push(format!(
                            "{path}: public field {} uses actingcommand_ledger storage types",
                            qualified(module, &name)
                        ));
                    }
                }
            }
            Item::Enum(item_enum) if is_public(&item_enum.vis) => {
                for variant in &item_enum.variants {
                    for (index, field) in variant.fields.iter().enumerate() {
                        if !type_uses_json_value(&field.ty, &aliases) {
                            continue;
                        }
                        let field_name = field
                            .ident
                            .as_ref()
                            .map_or_else(|| index.to_string(), ToString::to_string);
                        let name = format!("{}::{}::{field_name}", item_enum.ident, variant.ident);
                        violations.push(format!(
                            "{path}: public enum payload {} uses serde_json::Value",
                            qualified(module, &name)
                        ));
                    }
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

// Workflow #310 work package B: dependency faces and purity of the pure decision crates, the
// RuntimeOperation origin categories, the instance fact publication path, and provider ABI
// symbol literals.

/// Checks each named workspace package's direct dependencies on other workspace packages
/// against its allow-list, on the resolve graph of one `cargo metadata` document. Normal, dev
/// and build edges all count; packages outside the workspace are not part of the table. A named
/// package that does not resolve to exactly one workspace member is an error, not a pass.
pub fn workspace_dependency_allow_list_violations(
    metadata: &str,
    allow_lists: &[(&str, &[&str])],
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
        .collect::<Result<HashSet<_>, _>>()?;
    let mut package_names = HashMap::new();
    for package in packages {
        package_names.insert(
            required_field_string(package, "id")?,
            required_field_string(package, "name")?,
        );
    }
    let nodes = document
        .pointer("/resolve/nodes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "cargo metadata is missing resolve.nodes".to_string())?;
    let mut dependencies = HashMap::new();
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

    let mut violations = Vec::new();
    for (package, allowed) in allow_lists {
        let ids = workspace_members
            .iter()
            .filter(|id| package_names.get(*id).is_some_and(|name| name == package))
            .collect::<Vec<_>>();
        let [id] = ids.as_slice() else {
            return Err(format!(
                "workspace package {package} resolves to {} workspace members",
                ids.len()
            ));
        };
        let edges = dependencies
            .get(*id)
            .ok_or_else(|| format!("cargo metadata has no resolve node for {package}"))?;
        for dependency in edges {
            if !workspace_members.contains(dependency) {
                continue;
            }
            let name = package_names.get(dependency).ok_or_else(|| {
                format!("cargo metadata node has unknown package id {dependency}")
            })?;
            if !allowed.contains(&name.as_str()) {
                violations.push(format!(
                    "{package} depends on workspace package {name} outside its allow-list [{}]",
                    allowed.join(", ")
                ));
            }
        }
    }
    violations.sort();
    violations.dedup();
    Ok(violations)
}

/// Finds side-effect authority named by the production items of a pure decision module: a path
/// under `std::fs` or `std::net`, `SystemTime`, `Instant::now`, or a path into the ledger or
/// device crate. `use` trees are expanded (globs included) and macro bodies are read as token
/// paths. Items outside production by their cfg scope (`#[cfg(test)]` and the like) are dropped
/// first, so test code may name these freely. This text reading supplements the resolved-path
/// clippy bans of the same crates. Each violation is `path:line: <label>`.
pub fn inspect_pure_decision_source(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    if !ledger_owners::production_attributes(&file.attrs)? {
        return Ok(Vec::new());
    }
    let items = ledger_owners::production_items(&file.items)?;
    let mut visitor = SideEffectPathVisitor {
        path,
        violations: Vec::new(),
    };
    for item in &items {
        visitor.visit_item(item);
    }
    visitor.violations.dedup();
    Ok(visitor.violations)
}

struct SideEffectPathVisitor<'a> {
    path: &'a str,
    violations: Vec<String>,
}

impl SideEffectPathVisitor<'_> {
    fn check(&mut self, segments: &[String], span: Span) {
        if let Some(label) = side_effect_label(segments) {
            self.violations
                .push(format!("{}:{}: {label}", self.path, span.start().line));
        }
    }
}

fn side_effect_label(segments: &[String]) -> Option<&'static str> {
    let names = segments.iter().map(String::as_str).collect::<Vec<_>>();
    match names.as_slice() {
        ["std", "fs", ..] => Some("std::fs"),
        ["std", "net", ..] => Some("std::net"),
        ["actingcommand_ledger", ..] => Some("ledger crate path"),
        ["actingcommand_device", ..] => Some("device crate path"),
        _ if names.contains(&"SystemTime") => Some("std::time::SystemTime"),
        _ if names.windows(2).any(|pair| pair == ["Instant", "now"]) => Some("Instant::now"),
        _ => None,
    }
}

fn collect_use_leaves(
    prefix: &mut Vec<String>,
    tree: &UseTree,
    leaves: &mut Vec<(Vec<String>, Span)>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use_leaves(prefix, &path.tree, leaves);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let mut full = prefix.clone();
            full.push(name.ident.to_string());
            leaves.push((full, name.ident.span()));
        }
        UseTree::Rename(rename) => {
            let mut full = prefix.clone();
            full.push(rename.ident.to_string());
            leaves.push((full, rename.ident.span()));
        }
        UseTree::Glob(glob) => leaves.push((prefix.clone(), glob.star_token.spans[0])),
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use_leaves(prefix, item, leaves);
            }
        }
    }
}

/// Every `ident(::ident)*` run of a flattened token stream, with the span of its first ident.
fn token_paths(flat: &[TokenTree]) -> Vec<(Vec<String>, Span)> {
    let mut paths = Vec::new();
    let mut index = 0;
    while index < flat.len() {
        let TokenTree::Ident(first) = &flat[index] else {
            index += 1;
            continue;
        };
        let mut segments = vec![first.to_string()];
        let mut next = index + 1;
        while let (
            Some(TokenTree::Punct(colon)),
            Some(TokenTree::Punct(second)),
            Some(TokenTree::Ident(segment)),
        ) = (flat.get(next), flat.get(next + 1), flat.get(next + 2))
        {
            if colon.as_char() != ':' || second.as_char() != ':' {
                break;
            }
            segments.push(segment.to_string());
            next += 3;
        }
        paths.push((segments, first.span()));
        index = next;
    }
    paths
}

fn path_names(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect()
}

impl<'ast> Visit<'ast> for SideEffectPathVisitor<'_> {
    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let mut leaves = Vec::new();
        collect_use_leaves(&mut Vec::new(), &item.tree, &mut leaves);
        for (segments, span) in leaves {
            self.check(&segments, span);
        }
        syn::visit::visit_item_use(self, item);
    }

    fn visit_item_extern_crate(&mut self, item: &'ast syn::ItemExternCrate) {
        self.check(&[item.ident.to_string()], item.ident.span());
        syn::visit::visit_item_extern_crate(self, item);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if let Some(first) = path.segments.first() {
            self.check(&path_names(path), first.ident.span());
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let mut flat = Vec::new();
        flatten_tokens(mac.tokens.clone(), &mut flat);
        for (segments, span) in token_paths(&flat) {
            self.check(&segments, span);
        }
        syn::visit::visit_macro(self, mac);
    }
}

/// Finds every `allow` / `expect` attribute, direct or under `cfg_attr`, test code included,
/// that silences clippy's `disallowed_methods` / `disallowed_types` lints, also through
/// `clippy::style`, `clippy::all` or `warnings`. Each finding is `path:line: <level>(<lint>)`;
/// the caller names every one it accepts.
pub fn inspect_disallowed_lint_escapes(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut visitor = LintEscapeVisitor {
        path,
        escapes: Vec::new(),
        error: None,
    };
    visitor.visit_file(&file);
    if let Some(error) = visitor.error {
        return Err(error);
    }
    Ok(visitor.escapes)
}

struct LintEscapeVisitor<'a> {
    path: &'a str,
    escapes: Vec<String>,
    error: Option<String>,
}

impl<'ast> Visit<'ast> for LintEscapeVisitor<'_> {
    fn visit_attribute(&mut self, attribute: &'ast syn::Attribute) {
        match lint_escapes(&attribute.meta) {
            Ok(lints) => {
                let line = attribute.pound_token.spans[0].start().line;
                for lint in lints {
                    self.escapes.push(format!("{}:{line}: {lint}", self.path));
                }
            }
            Err(error) => {
                self.error.get_or_insert(format!("{}: {error}", self.path));
            }
        }
        syn::visit::visit_attribute(self, attribute);
    }
}

fn nested_metas(list: &syn::MetaList) -> Result<Vec<syn::Meta>, String> {
    syn::parse::Parser::parse2(
        syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
        list.tokens.clone(),
    )
    .map(|metas| metas.into_iter().collect())
    .map_err(|error| {
        format!(
            "unparsable {} attribute: {error}",
            path_names(&list.path).join("::")
        )
    })
}

fn lint_escapes(meta: &syn::Meta) -> Result<Vec<String>, String> {
    let syn::Meta::List(list) = meta else {
        return Ok(Vec::new());
    };
    if list.path.is_ident("cfg_attr") {
        let mut escapes = Vec::new();
        for attribute in nested_metas(list)?.iter().skip(1) {
            escapes.extend(lint_escapes(attribute)?);
        }
        return Ok(escapes);
    }
    let level = if list.path.is_ident("allow") {
        "allow"
    } else if list.path.is_ident("expect") {
        "expect"
    } else {
        return Ok(Vec::new());
    };
    Ok(nested_metas(list)?
        .iter()
        .filter_map(|lint| {
            let names = path_names(lint.path());
            let silences = matches!(
                names
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice(),
                ["warnings"]
                    | [
                        "clippy",
                        "disallowed_methods" | "disallowed_types" | "style" | "all"
                    ]
            );
            silences.then(|| format!("{level}({})", names.join("::")))
        })
        .collect())
}

/// One refusal branch of a gate function, as [`inspect_refusal_branches`] reads it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RefusalBranch {
    /// The `<classifier>::Variant` names the branch selects, sorted.
    pub variants: Vec<String>,
    /// The refusal codes the branch returns, sorted.
    pub codes: Vec<String>,
    /// The origin terms the branch tests, sorted: `EventActor::*` / `EventSource::*` values,
    /// `valid_*_origin` predicates, and `self.<field>` reads other than the request's own
    /// `actor` / `source` / `operation`.
    pub terms: Vec<String>,
}

/// Lists the production variants of the one enum named `enum_name`, in declaration order.
pub fn inspect_enum_variants(
    path: &str,
    source: &str,
    enum_name: &str,
) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let mut all = Vec::new();
    collect_nested_items(&items, &mut all);
    let enums = all
        .iter()
        .filter_map(|item| match item {
            Item::Enum(item_enum) if item_enum.ident == enum_name => Some(item_enum),
            _ => None,
        })
        .collect::<Vec<_>>();
    let [item_enum] = enums.as_slice() else {
        return Err(format!(
            "{path}: expected one enum {enum_name}, found {}",
            enums.len()
        ));
    };
    let variants = item_enum
        .variants
        .iter()
        .map(|variant| variant.ident.to_string())
        .collect::<Vec<_>>();
    if variants.is_empty() {
        return Err(format!("{path}: enum {enum_name} has no variants"));
    }
    Ok(variants)
}

fn find_function_block(
    path: &str,
    items: &[Item],
    owner: Option<&str>,
    function: &str,
) -> Result<syn::Block, String> {
    let mut all = Vec::new();
    collect_nested_items(items, &mut all);
    let mut blocks = Vec::new();
    for item in all {
        match (owner, item) {
            (None, Item::Fn(item_fn)) if item_fn.sig.ident == function => {
                blocks.push(item_fn.block.as_ref().clone());
            }
            (Some(owner), Item::Impl(item_impl))
                if impl_self_ident(item_impl).is_some_and(|ident| ident == owner) =>
            {
                for member in &item_impl.items {
                    if let syn::ImplItem::Fn(method) = member
                        && method.sig.ident == function
                    {
                        blocks.push(method.block.clone());
                    }
                }
            }
            _ => {}
        }
    }
    let count = blocks.len();
    let mut blocks = blocks.into_iter();
    match (blocks.next(), count) {
        (Some(block), 1) => Ok(block),
        _ => Err(format!(
            "{path}: expected one production function {}{function}, found {count}",
            owner.map(|owner| format!("{owner}::")).unwrap_or_default()
        )),
    }
}

/// Extracts the refusal branches of `owner::function`, one per top-level statement of its body:
/// - `if <condition> { .. }` whose body returns a refusal code: its variants are the
///   `<classifier>::Variant` patterns of any `matches!` in the condition, and its terms are the
///   origin terms the condition reads;
/// - `let <binding> = match .. { <classifier>::Variant .. => .., _ => None }`: its variants are the
///   named arms; its codes and terms come from the later `if let .. = <binding>` statement. A
///   catch-all arm must yield `None`, so it classifies nothing.
///
/// Codes are string literals passed first to an `*Error::new|request|fatal` constructor. Every
/// `<classifier>::Variant` the body names must belong to a branch: a classification in any other
/// shape, a catch-all beside named variants, or a classifying `let` never consumed is an error.
pub fn inspect_refusal_branches(
    path: &str,
    source: &str,
    owner: &str,
    function: &str,
    classifier: &str,
) -> Result<Vec<RefusalBranch>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let block = find_function_block(path, &items, Some(owner), function)?;
    let location = format!("{path}: {owner}::{function}");
    let mut branches = Vec::new();
    let mut consumers = HashSet::new();
    for (index, statement) in block.stmts.iter().enumerate() {
        match statement {
            Stmt::Expr(Expr::If(expr_if), _) if !consumers.contains(&index) => {
                let variants = classifier_variants_in(&location, classifier, |visitor| {
                    visitor.visit_expr(&expr_if.cond)
                })?;
                let codes = refusal_codes(|visitor| visitor.visit_block(&expr_if.then_branch));
                if codes.is_empty() {
                    if !variants.is_empty() {
                        return Err(format!(
                            "{location} selects {} without a refusal code",
                            variants.join(", ")
                        ));
                    }
                    continue;
                }
                branches.push(RefusalBranch {
                    variants,
                    codes,
                    terms: origin_terms(|visitor| visitor.visit_expr(&expr_if.cond)),
                });
            }
            Stmt::Local(local) => {
                let Some(init) = &local.init else {
                    continue;
                };
                let Expr::Match(expr_match) = init.expr.as_ref() else {
                    continue;
                };
                let variants = classifying_match_variants(&location, expr_match, classifier)?;
                if variants.is_empty() {
                    continue;
                }
                let Some(binding) = pattern_ident(&local.pat) else {
                    return Err(format!(
                        "{location}: a classifying match must bind one name"
                    ));
                };
                let consumer = block.stmts.iter().enumerate().skip(index + 1).find_map(
                    |(position, statement)| match statement {
                        Stmt::Expr(Expr::If(expr_if), _)
                            if matches!(
                                expr_if.cond.as_ref(),
                                Expr::Let(expr_let)
                                    if matches!(
                                        expr_let.expr.as_ref(),
                                        Expr::Path(scrutinee) if scrutinee.path.is_ident(binding)
                                    )
                            ) =>
                        {
                            Some((position, expr_if))
                        }
                        _ => None,
                    },
                );
                let Some((position, consumer)) = consumer else {
                    return Err(format!(
                        "{location}: classifying binding {binding} is never consumed by a refusal"
                    ));
                };
                consumers.insert(position);
                let codes = refusal_codes(|visitor| visitor.visit_expr_if(consumer));
                if codes.is_empty() {
                    return Err(format!(
                        "{location}: classifying binding {binding} is consumed without a refusal code"
                    ));
                }
                branches.push(RefusalBranch {
                    variants,
                    codes,
                    terms: origin_terms(|visitor| visitor.visit_expr_if(consumer)),
                });
            }
            _ => {}
        }
    }
    let mut mentions = ClassifierMentionVisitor {
        classifier,
        variants: BTreeSetString::new(),
    };
    mentions.visit_block(&block);
    let classified = branches
        .iter()
        .flat_map(|branch| branch.variants.iter().cloned())
        .collect::<BTreeSetString>();
    let unaccounted = mentions
        .variants
        .difference(&classified)
        .cloned()
        .collect::<Vec<_>>();
    if !unaccounted.is_empty() {
        return Err(format!(
            "{location} names {} outside a recognised refusal branch",
            unaccounted.join(", ")
        ));
    }
    if branches.is_empty() {
        return Err(format!("{location} has no refusal branch"));
    }
    Ok(branches)
}

type BTreeSetString = std::collections::BTreeSet<String>;

/// Lists the origin terms (see [`RefusalBranch::terms`]) that one production free function's
/// body reads, sorted.
pub fn inspect_function_origin_terms(
    path: &str,
    source: &str,
    function: &str,
) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let block = find_function_block(path, &items, None, function)?;
    let terms = origin_terms(|visitor| visitor.visit_block(&block));
    if terms.is_empty() {
        return Err(format!("{path}: {function} reads no origin term"));
    }
    Ok(terms)
}

fn classifier_variant(pattern: &Pat, classifier: &str) -> Option<String> {
    let path = match pattern {
        Pat::Path(value) => &value.path,
        Pat::Struct(value) => &value.path,
        Pat::TupleStruct(value) => &value.path,
        Pat::Paren(value) => return classifier_variant(&value.pat, classifier),
        _ => return None,
    };
    match path_names(path).as_slice() {
        [.., owner, variant] if owner == classifier => Some(variant.clone()),
        _ => None,
    }
}

/// Adds the classifier variants one pattern names; a catch-all case beside them is an error.
fn collect_classifier_pattern(
    pattern: &Pat,
    classifier: &str,
    variants: &mut BTreeSetString,
) -> Result<(), String> {
    let cases = match pattern {
        Pat::Or(value) => value.cases.iter().collect::<Vec<_>>(),
        other => vec![other],
    };
    let mut named = Vec::new();
    let mut catch_all = false;
    for case in cases {
        match classifier_variant(case, classifier) {
            Some(variant) => named.push(variant),
            None => catch_all |= matches!(case, Pat::Wild(_) | Pat::Ident(_)),
        }
    }
    if !named.is_empty() && catch_all {
        return Err(format!(
            "catch-all pattern beside {classifier} variants {}",
            named.join(", ")
        ));
    }
    variants.extend(named);
    Ok(())
}

fn matches_pattern(mac: &syn::Macro) -> Result<Pat, String> {
    syn::parse::Parser::parse2(
        |input: syn::parse::ParseStream<'_>| {
            input.parse::<Expr>()?;
            input.parse::<syn::Token![,]>()?;
            let pattern = Pat::parse_multi_with_leading_vert(input)?;
            if input.peek(syn::Token![if]) {
                input.parse::<syn::Token![if]>()?;
                input.parse::<Expr>()?;
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
            Ok(pattern)
        },
        mac.tokens.clone(),
    )
    .map_err(|error| format!("unparsable matches! invocation: {error}"))
}

struct ClassifierPatternVisitor<'a> {
    classifier: &'a str,
    variants: BTreeSetString,
    error: Option<String>,
}

impl<'ast> Visit<'ast> for ClassifierPatternVisitor<'_> {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac.path.is_ident("matches") {
            let collected = matches_pattern(mac).and_then(|pattern| {
                collect_classifier_pattern(&pattern, self.classifier, &mut self.variants)
            });
            if let Err(error) = collected {
                self.error.get_or_insert(error);
            }
        }
        syn::visit::visit_macro(self, mac);
    }
}

fn classifier_variants_in(
    location: &str,
    classifier: &str,
    visit: impl FnOnce(&mut ClassifierPatternVisitor<'_>),
) -> Result<Vec<String>, String> {
    let mut visitor = ClassifierPatternVisitor {
        classifier,
        variants: BTreeSetString::new(),
        error: None,
    };
    visit(&mut visitor);
    match visitor.error {
        Some(error) => Err(format!("{location}: {error}")),
        None => Ok(visitor.variants.into_iter().collect()),
    }
}

fn classifying_match_variants(
    location: &str,
    expr_match: &ExprMatch,
    classifier: &str,
) -> Result<Vec<String>, String> {
    let mut variants = BTreeSetString::new();
    let mut catch_all_bodies = Vec::new();
    for arm in &expr_match.arms {
        let before = variants.len();
        collect_classifier_pattern(&arm.pat, classifier, &mut variants)
            .map_err(|error| format!("{location}: {error}"))?;
        if variants.len() == before && matches!(arm.pat, Pat::Wild(_) | Pat::Ident(_)) {
            catch_all_bodies.push(arm.body.as_ref());
        }
    }
    if !variants.is_empty()
        && catch_all_bodies
            .iter()
            .any(|body| !matches!(body, Expr::Path(value) if value.path.is_ident("None")))
    {
        return Err(format!(
            "{location}: a catch-all arm beside {classifier} variants must yield None"
        ));
    }
    Ok(variants.into_iter().collect())
}

struct ClassifierMentionVisitor<'a> {
    classifier: &'a str,
    variants: BTreeSetString,
}

impl ClassifierMentionVisitor<'_> {
    fn mention(&mut self, segments: &[String]) {
        if let [.., owner, variant] = segments
            && owner == self.classifier
        {
            self.variants.insert(variant.clone());
        }
    }
}

impl<'ast> Visit<'ast> for ClassifierMentionVisitor<'_> {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.mention(&path_names(path));
        syn::visit::visit_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let mut flat = Vec::new();
        flatten_tokens(mac.tokens.clone(), &mut flat);
        for (segments, _) in token_paths(&flat) {
            self.mention(&segments);
        }
        syn::visit::visit_macro(self, mac);
    }
}

struct RefusalCodeVisitor {
    codes: BTreeSetString,
}

impl<'ast> Visit<'ast> for RefusalCodeVisitor {
    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let Expr::Path(function) = call.func.as_ref()
            && let [.., owner, constructor] = path_names(&function.path).as_slice()
            && owner.ends_with("Error")
            && matches!(constructor.as_str(), "new" | "request" | "fatal")
            && let Some(Expr::Lit(syn::ExprLit {
                lit: Lit::Str(code),
                ..
            })) = call.args.first()
        {
            self.codes.insert(code.value());
        }
        syn::visit::visit_expr_call(self, call);
    }
}

fn refusal_codes(visit: impl FnOnce(&mut RefusalCodeVisitor)) -> Vec<String> {
    let mut visitor = RefusalCodeVisitor {
        codes: BTreeSetString::new(),
    };
    visit(&mut visitor);
    visitor.codes.into_iter().collect()
}

struct OriginTermVisitor {
    terms: BTreeSetString,
}

impl OriginTermVisitor {
    fn path_term(&mut self, segments: &[String]) {
        if let [.., kind, value] = segments
            && matches!(kind.as_str(), "EventActor" | "EventSource")
        {
            self.terms.insert(format!("{kind}::{value}"));
        }
        if let Some(last) = segments.last()
            && last.starts_with("valid_")
            && last.ends_with("_origin")
        {
            self.terms.insert(last.clone());
        }
    }

    fn field_term(&mut self, field: &str) {
        if !matches!(field, "actor" | "source" | "operation") {
            self.terms.insert(format!("self.{field}"));
        }
    }
}

impl<'ast> Visit<'ast> for OriginTermVisitor {
    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.path_term(&path_names(&node.path));
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_expr_field(&mut self, node: &'ast syn::ExprField) {
        if let Expr::Path(base) = node.base.as_ref()
            && base.path.is_ident("self")
            && let syn::Member::Named(field) = &node.member
        {
            self.field_term(&field.to_string());
        }
        syn::visit::visit_expr_field(self, node);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let mut flat = Vec::new();
        flatten_tokens(mac.tokens.clone(), &mut flat);
        for (segments, _) in token_paths(&flat) {
            self.path_term(&segments);
        }
        for window in flat.windows(3) {
            if let [
                TokenTree::Ident(base),
                TokenTree::Punct(dot),
                TokenTree::Ident(field),
            ] = window
                && base == "self"
                && dot.as_char() == '.'
            {
                self.field_term(&field.to_string());
            }
        }
        syn::visit::visit_macro(self, mac);
    }
}

fn origin_terms(visit: impl FnOnce(&mut OriginTermVisitor)) -> Vec<String> {
    let mut visitor = OriginTermVisitor {
        terms: BTreeSetString::new(),
    };
    visit(&mut visitor);
    visitor.terms.into_iter().collect()
}

/// The write API of a store type, as [`inspect_store_write_api`] reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreWriteApi {
    /// `&mut self` methods visible outside their module, sorted.
    pub mutating_methods: Vec<String>,
    /// Receiver-less associated functions returning the store, visible outside their module.
    pub constructors: Vec<String>,
}

/// Reads the write API of `type_name` from its inherent production impls: the `&mut self`
/// methods and the receiver-less functions returning `Self` that are visible outside the
/// defining module. A type without such an impl or without a mutating method is an error.
pub fn inspect_store_write_api(
    path: &str,
    source: &str,
    type_name: &str,
) -> Result<StoreWriteApi, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let mut all = Vec::new();
    collect_nested_items(&items, &mut all);
    let mut api = StoreWriteApi {
        mutating_methods: Vec::new(),
        constructors: Vec::new(),
    };
    for item in all {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        if item_impl.trait_.is_some()
            || !impl_self_ident(item_impl).is_some_and(|ident| ident == type_name)
        {
            continue;
        }
        for member in &item_impl.items {
            let syn::ImplItem::Fn(method) = member else {
                continue;
            };
            if matches!(method.vis, Visibility::Inherited) {
                continue;
            }
            let name = method.sig.ident.to_string();
            match method.sig.receiver() {
                Some(receiver) => {
                    if matches!(receiver.ty.as_ref(), Type::Reference(reference) if reference.mutability.is_some())
                    {
                        api.mutating_methods.push(name);
                    }
                }
                None => {
                    if signature_returns_ident(&method.sig, &["Self", type_name]) {
                        api.constructors.push(name);
                    }
                }
            }
        }
    }
    if api.mutating_methods.is_empty() {
        return Err(format!(
            "{path}: {type_name} exposes no mutating method outside its module"
        ));
    }
    api.mutating_methods.sort();
    api.constructors.sort();
    Ok(api)
}

/// Lists the production sites in one source file that write store state: calls of the store's
/// mutating methods and constructors (`api`, from [`inspect_store_write_api`]) and every
/// `<payload_type>::*` construction, since a published or invalidated fact becomes store state
/// on the next synchronisation. The store's own inherent impl is its implementation and is not
/// listed. Each row is `path::Owner::function -> target [<gate_field> held]` or
/// `[no <gate_field>]`: held when a binding of `lock(&self.<gate_field>, ..)` made earlier in an
/// enclosing block is still alive (not passed to `drop`).
pub fn inspect_store_writes(
    path: &str,
    source: &str,
    store: &str,
    api: &StoreWriteApi,
    payload_type: &str,
    gate_field: &str,
) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    if !ledger_owners::production_attributes(&file.attrs)? {
        return Ok(Vec::new());
    }
    let items = ledger_owners::production_items(&file.items)?;
    let mut visitor = StoreWriteVisitor {
        path,
        store,
        api,
        payload_type,
        gate_field,
        scope: FunctionScope::default(),
        gates: Vec::new(),
        rows: Vec::new(),
    };
    for item in &items {
        visitor.visit_item(item);
    }
    visitor.rows.sort();
    visitor.rows.dedup();
    Ok(visitor.rows)
}

#[derive(Default)]
struct FunctionScope {
    owner: Option<String>,
    function: Option<String>,
}

impl FunctionScope {
    fn name(&self) -> String {
        match (&self.owner, &self.function) {
            (Some(owner), Some(function)) => format!("{owner}::{function}"),
            (None, Some(function)) => function.clone(),
            (_, None) => "<item>".to_string(),
        }
    }
}

struct StoreWriteVisitor<'a> {
    path: &'a str,
    store: &'a str,
    api: &'a StoreWriteApi,
    payload_type: &'a str,
    gate_field: &'a str,
    scope: FunctionScope,
    gates: Vec<Vec<String>>,
    rows: Vec<String>,
}

impl StoreWriteVisitor<'_> {
    fn record(&mut self, target: String) {
        let gate = if self.gates.iter().any(|scope| !scope.is_empty()) {
            format!("{} held", self.gate_field)
        } else {
            format!("no {}", self.gate_field)
        };
        self.rows.push(format!(
            "{}::{} -> {target} [{gate}]",
            self.path,
            self.scope.name()
        ));
    }

    fn path_target(&self, segments: &[String]) -> Option<String> {
        let [.., owner, name] = segments else {
            return None;
        };
        ((owner == self.store && self.api.constructors.contains(name))
            || owner == self.payload_type)
            .then(|| format!("{owner}::{name}"))
    }
}

/// True when `expr` contains a call `lock(&self.<field>, ..)`.
fn locks_field(expr: &Expr, field: &str) -> bool {
    struct LockVisitor<'a> {
        field: &'a str,
        found: bool,
    }
    impl<'ast> Visit<'ast> for LockVisitor<'_> {
        fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
            if matches!(call.func.as_ref(), Expr::Path(function) if function.path.is_ident("lock"))
                && let Some(Expr::Reference(reference)) = call.args.first()
                && is_self_field(&reference.expr, self.field)
            {
                self.found = true;
            }
            syn::visit::visit_expr_call(self, call);
        }
    }
    let mut visitor = LockVisitor {
        field,
        found: false,
    };
    visitor.visit_expr(expr);
    visitor.found
}

fn is_self_field(expr: &Expr, field: &str) -> bool {
    matches!(expr, Expr::Field(value) if is_self_field_access(value, field))
}

fn is_self_field_access(value: &syn::ExprField, field: &str) -> bool {
    matches!(value.base.as_ref(), Expr::Path(base) if base.path.is_ident("self"))
        && matches!(&value.member, syn::Member::Named(name) if name == field)
}

fn local_binding(pattern: &Pat) -> Option<String> {
    match pattern {
        Pat::Ident(value) => Some(value.ident.to_string()),
        Pat::Type(value) => local_binding(&value.pat),
        _ => None,
    }
}

impl<'ast> Visit<'ast> for StoreWriteVisitor<'_> {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let owner = impl_self_ident(node).map(ToString::to_string);
        if node.trait_.is_none() && owner.as_deref() == Some(self.store) {
            return;
        }
        let previous = std::mem::replace(&mut self.scope.owner, owner);
        syn::visit::visit_item_impl(self, node);
        self.scope.owner = previous;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let previous = self.scope.function.replace(node.sig.ident.to_string());
        let gates = std::mem::take(&mut self.gates);
        syn::visit::visit_impl_item_fn(self, node);
        self.gates = gates;
        self.scope.function = previous;
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let owner = self.scope.owner.take();
        let previous = self.scope.function.replace(node.sig.ident.to_string());
        let gates = std::mem::take(&mut self.gates);
        syn::visit::visit_item_fn(self, node);
        self.gates = gates;
        self.scope.function = previous;
        self.scope.owner = owner;
    }

    fn visit_block(&mut self, block: &'ast syn::Block) {
        self.gates.push(Vec::new());
        for statement in &block.stmts {
            self.visit_stmt(statement);
            match statement {
                Stmt::Local(local) => {
                    if let Some(init) = &local.init
                        && locks_field(&init.expr, self.gate_field)
                        && let Some(binding) = local_binding(&local.pat)
                        && binding != "_"
                        && let Some(scope) = self.gates.last_mut()
                    {
                        scope.push(binding);
                    }
                }
                Stmt::Expr(Expr::Call(call), _) => {
                    if matches!(call.func.as_ref(), Expr::Path(function) if function.path.is_ident("drop"))
                        && let Some(Expr::Path(argument)) = call.args.first()
                        && let Some(name) = argument.path.get_ident()
                    {
                        for scope in &mut self.gates {
                            scope.retain(|binding| name != binding);
                        }
                    }
                }
                _ => {}
            }
        }
        self.gates.pop();
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if self.api.mutating_methods.contains(&method) {
            self.record(method);
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Expr::Path(function) = node.func.as_ref()
            && let Some(target) = self.path_target(&path_names(&function.path))
        {
            self.record(target);
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let mut flat = Vec::new();
        flatten_tokens(mac.tokens.clone(), &mut flat);
        for window in flat.windows(2) {
            if let [TokenTree::Punct(dot), TokenTree::Ident(method)] = window
                && dot.as_char() == '.'
                && self.api.mutating_methods.contains(&method.to_string())
            {
                self.record(format!("{method} (in macro)"));
            }
        }
        for (segments, _) in token_paths(&flat) {
            if let Some(target) = self.path_target(&segments) {
                self.record(format!("{target} (in macro)"));
            }
        }
        syn::visit::visit_macro(self, mac);
    }
}

/// Lists, for each named `<classifier>::Variant`, the calls that the one match arm naming it
/// inside `owner::function` makes on `self`: `method` for a method of `self`, `field.method` for
/// a method of a `self` field, in source order. A variant without exactly one arm is an error.
pub fn inspect_dispatch_arm_calls(
    path: &str,
    source: &str,
    owner: &str,
    function: &str,
    classifier: &str,
    variants: &[&str],
) -> Result<Vec<(String, Vec<String>)>, String> {
    struct ArmFinder<'a> {
        classifier: &'a str,
        arms: Vec<(BTreeSetString, syn::Arm)>,
        error: Option<String>,
    }
    impl<'ast> Visit<'ast> for ArmFinder<'_> {
        fn visit_arm(&mut self, arm: &'ast syn::Arm) {
            let mut named = BTreeSetString::new();
            match collect_classifier_pattern(&arm.pat, self.classifier, &mut named) {
                Ok(()) if !named.is_empty() => self.arms.push((named, arm.clone())),
                Ok(()) => {}
                Err(error) => {
                    self.error.get_or_insert(error);
                }
            }
            syn::visit::visit_arm(self, arm);
        }
    }
    struct SelfCallVisitor {
        calls: Vec<String>,
    }
    impl<'ast> Visit<'ast> for SelfCallVisitor {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            match node.receiver.as_ref() {
                Expr::Path(receiver) if receiver.path.is_ident("self") => {
                    self.calls.push(node.method.to_string());
                }
                Expr::Field(receiver) if matches!(receiver.base.as_ref(), Expr::Path(base) if base.path.is_ident("self")) => {
                    if let syn::Member::Named(field) = &receiver.member {
                        self.calls.push(format!("{field}.{}", node.method));
                    }
                }
                _ => {}
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }

    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let block = find_function_block(path, &items, Some(owner), function)?;
    let mut finder = ArmFinder {
        classifier,
        arms: Vec::new(),
        error: None,
    };
    finder.visit_block(&block);
    if let Some(error) = finder.error {
        return Err(format!("{path}: {owner}::{function}: {error}"));
    }
    let mut calls = Vec::new();
    for variant in variants {
        let arms = finder
            .arms
            .iter()
            .filter(|(named, _)| named.contains(*variant))
            .collect::<Vec<_>>();
        let [(_, arm)] = arms.as_slice() else {
            return Err(format!(
                "{path}: {owner}::{function} has {} arms for {classifier}::{variant}",
                arms.len()
            ));
        };
        let mut visitor = SelfCallVisitor { calls: Vec::new() };
        visitor.visit_arm(arm);
        calls.push(((*variant).to_string(), visitor.calls));
    }
    Ok(calls)
}

/// Lists every production access to `self.<field>` in one source file as
/// `path::Owner::function -> field.method`: the method called on the field itself or on
/// `lock(&self.<field>, ..)` (through `?`). Any other access is `field.<other>`.
pub fn inspect_field_accesses(
    path: &str,
    source: &str,
    field: &str,
) -> Result<Vec<String>, String> {
    struct FieldAccessVisitor<'a> {
        path: &'a str,
        field: &'a str,
        scope: FunctionScope,
        consumed: HashSet<*const syn::ExprField>,
        rows: Vec<String>,
    }
    impl FieldAccessVisitor<'_> {
        fn record(&mut self, access: &str) {
            self.rows.push(format!(
                "{}::{} -> {}.{access}",
                self.path,
                self.scope.name(),
                self.field
            ));
        }
    }
    fn peel(expr: &Expr) -> &Expr {
        match expr {
            Expr::Try(value) => peel(&value.expr),
            Expr::Paren(value) => peel(&value.expr),
            other => other,
        }
    }
    impl<'ast> Visit<'ast> for FieldAccessVisitor<'_> {
        fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
            let owner = impl_self_ident(node).map(ToString::to_string);
            let previous = std::mem::replace(&mut self.scope.owner, owner);
            syn::visit::visit_item_impl(self, node);
            self.scope.owner = previous;
        }

        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            let previous = self.scope.function.replace(node.sig.ident.to_string());
            syn::visit::visit_impl_item_fn(self, node);
            self.scope.function = previous;
        }

        fn visit_item_fn(&mut self, node: &'ast ItemFn) {
            let owner = self.scope.owner.take();
            let previous = self.scope.function.replace(node.sig.ident.to_string());
            syn::visit::visit_item_fn(self, node);
            self.scope.function = previous;
            self.scope.owner = owner;
        }

        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            let receiver = match peel(&node.receiver) {
                Expr::Field(value) => Some(value),
                Expr::Call(call) if matches!(call.func.as_ref(), Expr::Path(function) if function.path.is_ident("lock")) => {
                    match call.args.first() {
                        Some(Expr::Reference(reference)) => match reference.expr.as_ref() {
                            Expr::Field(value) => Some(value),
                            _ => None,
                        },
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some(value) = receiver
                && is_self_field_access(value, self.field)
            {
                self.consumed.insert(value as *const syn::ExprField);
                self.record(&node.method.to_string());
            }
            syn::visit::visit_expr_method_call(self, node);
        }

        fn visit_expr_field(&mut self, node: &'ast syn::ExprField) {
            if !self.consumed.contains(&(node as *const syn::ExprField))
                && is_self_field_access(node, self.field)
            {
                self.record("<other>");
            }
            syn::visit::visit_expr_field(self, node);
        }
    }

    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    if !ledger_owners::production_attributes(&file.attrs)? {
        return Ok(Vec::new());
    }
    let items = ledger_owners::production_items(&file.items)?;
    let mut visitor = FieldAccessVisitor {
        path,
        field,
        scope: FunctionScope::default(),
        consumed: HashSet::new(),
        rows: Vec::new(),
    };
    for item in &items {
        visitor.visit_item(item);
    }
    visitor.rows.sort();
    visitor.rows.dedup();
    Ok(visitor.rows)
}

/// Finds string, byte-string and C-string literals whose value starts with `ac_` (a provider ABI
/// symbol name) anywhere in one source file, test code, attributes and macro bodies included:
/// provider symbol names have one source, the vision-ffi loader. Each violation is
/// `path:line: <literal>`.
pub fn inspect_provider_symbol_literals(path: &str, source: &str) -> Result<Vec<String>, String> {
    syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let tokens = source
        .parse::<TokenStream>()
        .map_err(|err| format!("failed to tokenize {path}: {err}"))?;
    let mut flat = Vec::new();
    flatten_tokens(tokens, &mut flat);
    let mut violations = Vec::new();
    for token in flat {
        let TokenTree::Literal(literal) = token else {
            continue;
        };
        let value = match Lit::new(literal.clone()) {
            Lit::Str(value) => value.value().into_bytes(),
            Lit::ByteStr(value) => value.value(),
            Lit::CStr(value) => value.value().into_bytes(),
            _ => continue,
        };
        if value.starts_with(b"ac_") {
            violations.push(format!(
                "{path}:{}: provider symbol literal {literal}",
                literal.span().start().line
            ));
        }
    }
    Ok(violations)
}

// Workflow #310 work package B, second half (slice gB2): the capacity admission structure, the
// planning document decoding responsibility, the database / forensic / vendor stdio ownership
// and the typed host split. The inspectors below read production functions as `FunctionFacts`
// and state each boundary as a call relation, a construction site or a declaration.

/// A definition's declared visibility, as written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeclaredVisibility {
    /// No visibility: its own module and that module's children.
    Private,
    /// `pub(super)`: the parent module and the parent's other children.
    Super,
    /// `pub(crate)`.
    Crate,
    /// `pub`.
    Public,
    /// `pub(self)` or `pub(in <path>)`.
    Restricted,
}

fn declared_visibility(visibility: &Visibility) -> DeclaredVisibility {
    match visibility {
        Visibility::Inherited => DeclaredVisibility::Private,
        Visibility::Public(_) => DeclaredVisibility::Public,
        Visibility::Restricted(restricted) if restricted.in_token.is_none() => {
            if restricted.path.is_ident("crate") {
                DeclaredVisibility::Crate
            } else if restricted.path.is_ident("super") {
                DeclaredVisibility::Super
            } else {
                DeclaredVisibility::Restricted
            }
        }
        Visibility::Restricted(_) => DeclaredVisibility::Restricted,
    }
}

/// One reference a production function body makes, as [`inspect_source_facts`] reads it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum FunctionReference {
    /// A call through a path, `a::b(..)`: the path segments.
    Call(Vec<String>),
    /// A method call `<receiver>.method(..)`: the receiver when it is a name (`self` or a local)
    /// or a `self` field (`self.field`), and the method.
    Method(Option<String>, String),
    /// A struct literal `a::B { .. }`: the path segments.
    Struct(Vec<String>),
    /// A path read as a value: a multi-segment path (`Enum::Unit`) or an upper-case name.
    Value(Vec<String>),
    /// A path a pattern names (`Enum::Variant { .. }` in a `match` arm, `let` or `matches!`).
    Pattern(Vec<String>),
    /// A read of `self.<field>`.
    SelfField(String),
    /// A string literal outside patterns and attributes.
    Literal(String),
}

/// Whether `reference` is what `target` names:
/// - `"text"`: that string literal;
/// - `receiver.member`: a method `member` called on `receiver` (`self.member` also matches a
///   read of that `self` field); `.member` matches the method on any receiver;
/// - `name`: a call through a path ending in `name`, or a method `name`;
/// - `a::b`: a call, struct literal, value or pattern whose path ends in those segments.
pub fn reference_matches(reference: &FunctionReference, target: &str) -> bool {
    if let Some(literal) = target
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        return matches!(reference, FunctionReference::Literal(value) if value == literal);
    }
    if !target.contains("::")
        && let Some((receiver, member)) = target.rsplit_once('.')
    {
        return match reference {
            FunctionReference::Method(actual, method) => {
                method == member && (receiver.is_empty() || actual.as_deref() == Some(receiver))
            }
            FunctionReference::SelfField(field) => receiver == "self" && field == member,
            _ => false,
        };
    }
    let segments = target.split("::").collect::<Vec<_>>();
    match reference {
        FunctionReference::Method(_, method) => segments.len() == 1 && method == segments[0],
        FunctionReference::Call(path)
        | FunctionReference::Struct(path)
        | FunctionReference::Value(path)
        | FunctionReference::Pattern(path) => path_ends_with(path, &segments),
        FunctionReference::SelfField(_) | FunctionReference::Literal(_) => false,
    }
}

/// Whether `reference` calls `callee`: `name` matches a call through a path ending in `name`
/// and a method `name`; `Type::name` matches a call through a path ending in both segments.
pub fn reference_calls(reference: &FunctionReference, callee: &str) -> bool {
    let segments = callee.split("::").collect::<Vec<_>>();
    match reference {
        FunctionReference::Call(path) => path_ends_with(path, &segments),
        FunctionReference::Method(_, method) => segments.len() == 1 && method == segments[0],
        _ => false,
    }
}

fn path_ends_with(path: &[String], segments: &[&str]) -> bool {
    path.len() >= segments.len()
        && path[path.len() - segments.len()..]
            .iter()
            .zip(segments)
            .all(|(actual, expected)| actual == expected)
}

/// One production function or method of a source file, as [`inspect_source_facts`] reads it.
#[derive(Debug, Clone)]
pub struct FunctionFacts {
    /// The file, as given to [`inspect_source_facts`].
    pub path: String,
    /// The type of the `impl` (or the trait of a default method) the function belongs to.
    pub owner: Option<String>,
    /// The trait of a trait `impl`.
    pub trait_name: Option<String>,
    pub name: String,
    pub visibility: DeclaredVisibility,
    pub line: usize,
    /// Every type name the signature mentions.
    pub signature_types: std::collections::BTreeSet<String>,
    /// Every reference of the body in source order, with its line.
    pub references: Vec<(FunctionReference, usize)>,
    /// The file-system mutations the body makes itself, with the line: `File::create*`, the
    /// `std::fs` writes, copies, links, renames, removals and directory creations, and
    /// `OpenOptions(<flags>).open` chains that write, create, truncate or append.
    pub file_writes: Vec<(String, usize)>,
}

impl FunctionFacts {
    /// `Owner::name` for a method, `name` for a free function.
    pub fn qualified_name(&self) -> String {
        match &self.owner {
            Some(owner) => format!("{owner}::{}", self.name),
            None => self.name.clone(),
        }
    }

    /// `path::Owner::name`.
    pub fn site(&self) -> String {
        format!("{}::{}", self.path, self.qualified_name())
    }

    /// Whether the body makes a reference [`reference_matches`] `target`.
    pub fn references_target(&self, target: &str) -> bool {
        self.references
            .iter()
            .any(|(reference, _)| reference_matches(reference, target))
    }

    /// Whether the body calls `callee` ([`reference_calls`]).
    pub fn calls(&self, callee: &str) -> bool {
        self.references
            .iter()
            .any(|(reference, _)| reference_calls(reference, callee))
    }
}

/// The production facts of one source file.
#[derive(Debug, Clone)]
pub struct SourceFacts {
    pub path: String,
    pub functions: Vec<FunctionFacts>,
    /// Production `impl` blocks as (self type, trait).
    pub impls: Vec<(String, Option<String>)>,
    /// Types the production items define: structs, enums, unions, aliases and traits.
    pub types: std::collections::BTreeSet<String>,
    /// Out-of-line `mod name;` declarations with their visibility.
    pub modules: Vec<(String, DeclaredVisibility)>,
    /// Named fields of production structs as (struct, field, visibility).
    pub fields: Vec<(String, String, DeclaredVisibility)>,
}

impl SourceFacts {
    /// The types outside this file that its production `impl` blocks extend.
    pub fn extended_types(&self) -> std::collections::BTreeSet<String> {
        self.impls
            .iter()
            .map(|(owner, _)| owner.clone())
            .filter(|owner| !self.types.contains(owner))
            .collect()
    }
}

/// Reads the production functions of one source file: items outside production by their cfg
/// scope (`#[cfg(test)]` and the like, on items, statements, match arms and field values) are
/// dropped first. Macro bodies are read as comma-separated expressions when they parse so
/// (`format!`, `vec!`, `assert!`, the scrutinee and pattern of `matches!`), and as token paths
/// otherwise.
pub fn inspect_source_facts(path: &str, source: &str) -> Result<SourceFacts, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut facts = SourceFacts {
        path: path.to_string(),
        functions: Vec::new(),
        impls: Vec::new(),
        types: BTreeSetString::new(),
        modules: Vec::new(),
        fields: Vec::new(),
    };
    if !ledger_owners::production_attributes(&file.attrs)? {
        return Ok(facts);
    }
    let items = ledger_owners::production_items(&file.items)?;
    let mut visitor = FactsVisitor {
        path,
        owner: None,
        trait_name: None,
        stack: Vec::new(),
        facts: &mut facts,
        patterns: 0,
        error: None,
    };
    for item in &items {
        visitor.visit_item(item);
    }
    if let Some(error) = visitor.error {
        return Err(error);
    }
    Ok(facts)
}

struct FactsVisitor<'a> {
    path: &'a str,
    owner: Option<String>,
    trait_name: Option<String>,
    stack: Vec<FunctionFacts>,
    facts: &'a mut SourceFacts,
    patterns: usize,
    error: Option<String>,
}

impl FactsVisitor<'_> {
    fn begin(
        &mut self,
        owner: Option<String>,
        trait_name: Option<String>,
        signature: &syn::Signature,
        visibility: DeclaredVisibility,
    ) {
        struct TypeNames<'a>(&'a mut BTreeSetString);
        impl<'ast> Visit<'ast> for TypeNames<'_> {
            fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
                self.0.insert(segment.ident.to_string());
                syn::visit::visit_path_segment(self, segment);
            }
        }
        let mut signature_types = BTreeSetString::new();
        TypeNames(&mut signature_types).visit_signature(signature);
        self.stack.push(FunctionFacts {
            path: self.path.to_string(),
            owner,
            trait_name,
            name: signature.ident.to_string(),
            visibility,
            line: signature.ident.span().start().line,
            signature_types,
            references: Vec::new(),
            file_writes: Vec::new(),
        });
    }

    fn end(&mut self) {
        if let Some(function) = self.stack.pop() {
            self.facts.functions.push(function);
        }
    }

    fn record(&mut self, reference: FunctionReference, span: Span) {
        if let Some(function) = self.stack.last_mut() {
            function.references.push((reference, span.start().line));
        }
    }

    fn file_write(&mut self, label: String, span: Span) {
        if let Some(function) = self.stack.last_mut() {
            function.file_writes.push((label, span.start().line));
        }
    }

    fn production(&mut self, attributes: &[syn::Attribute]) -> bool {
        match ledger_owners::production_attributes(attributes) {
            Ok(production) => production,
            Err(error) => {
                self.error.get_or_insert(format!("{}: {error}", self.path));
                true
            }
        }
    }
}

fn expression_attributes(expr: &Expr) -> &[syn::Attribute] {
    match expr {
        Expr::Assign(value) => &value.attrs,
        Expr::Block(value) => &value.attrs,
        Expr::Call(value) => &value.attrs,
        Expr::ForLoop(value) => &value.attrs,
        Expr::If(value) => &value.attrs,
        Expr::Loop(value) => &value.attrs,
        Expr::Macro(value) => &value.attrs,
        Expr::Match(value) => &value.attrs,
        Expr::MethodCall(value) => &value.attrs,
        Expr::Unsafe(value) => &value.attrs,
        Expr::While(value) => &value.attrs,
        _ => &[],
    }
}

fn receiver_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Path(value) => value.path.get_ident().map(ToString::to_string),
        Expr::Field(value) => match (value.base.as_ref(), &value.member) {
            (Expr::Path(base), syn::Member::Named(field)) if base.path.is_ident("self") => {
                Some(format!("self.{field}"))
            }
            _ => None,
        },
        Expr::Reference(value) => receiver_name(&value.expr),
        Expr::Paren(value) => receiver_name(&value.expr),
        _ => None,
    }
}

fn file_write_call(names: &[String]) -> Option<String> {
    let names = names.iter().map(String::as_str).collect::<Vec<_>>();
    match names.as_slice() {
        [.., "File", operation @ ("create" | "create_new")] => Some(format!("File::{operation}")),
        [
            ..,
            "fs",
            operation @ ("write" | "copy" | "rename" | "hard_link" | "remove_file" | "remove_dir"
            | "remove_dir_all" | "create_dir" | "create_dir_all" | "set_permissions"),
        ] => Some(format!("fs::{operation}")),
        _ => None,
    }
}

/// The `write` / `append` / `create` / `create_new` / `truncate` flags of an
/// `OpenOptions::new()...` receiver chain that are not literally `false`, in call order.
fn open_options_flags(receiver: &Expr) -> Option<Vec<String>> {
    let mut flags = Vec::new();
    let mut expr = receiver;
    loop {
        match expr {
            Expr::MethodCall(call) => {
                let name = call.method.to_string();
                let disabled = matches!(
                    call.args.first(),
                    Some(Expr::Lit(syn::ExprLit { lit: Lit::Bool(value), .. })) if !value.value
                );
                if matches!(
                    name.as_str(),
                    "write" | "append" | "create" | "create_new" | "truncate"
                ) && !disabled
                {
                    flags.push(name);
                }
                expr = &call.receiver;
            }
            Expr::Paren(value) => expr = &value.expr,
            Expr::Call(call) => {
                let rooted = matches!(call.func.as_ref(), Expr::Path(function)
                    if path_ends_with(&path_names(&function.path), &["OpenOptions", "new"]));
                if !rooted || flags.is_empty() {
                    return None;
                }
                flags.reverse();
                return Some(flags);
            }
            _ => return None,
        }
    }
}

fn matches_parts(mac: &syn::Macro) -> Result<(Expr, Pat), String> {
    syn::parse::Parser::parse2(
        |input: syn::parse::ParseStream<'_>| {
            let scrutinee = input.parse::<Expr>()?;
            input.parse::<syn::Token![,]>()?;
            let pattern = Pat::parse_multi_with_leading_vert(input)?;
            if input.peek(syn::Token![if]) {
                input.parse::<syn::Token![if]>()?;
                input.parse::<Expr>()?;
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
            Ok((scrutinee, pattern))
        },
        mac.tokens.clone(),
    )
    .map_err(|error| format!("unparsable matches! invocation: {error}"))
}

fn is_upper_name(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

impl<'ast> Visit<'ast> for FactsVisitor<'_> {
    fn visit_attribute(&mut self, _: &'ast syn::Attribute) {}

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let name = node.ident.to_string();
        self.facts.types.insert(name.clone());
        if let syn::Fields::Named(fields) = &node.fields {
            for field in &fields.named {
                if let Some(ident) = &field.ident {
                    self.facts.fields.push((
                        name.clone(),
                        ident.to_string(),
                        declared_visibility(&field.vis),
                    ));
                }
            }
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        self.facts.types.insert(node.ident.to_string());
        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_union(&mut self, node: &'ast syn::ItemUnion) {
        self.facts.types.insert(node.ident.to_string());
        syn::visit::visit_item_union(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        self.facts.types.insert(node.ident.to_string());
        syn::visit::visit_item_type(self, node);
    }

    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        self.facts.types.insert(node.ident.to_string());
        let owner = self.owner.replace(node.ident.to_string());
        let trait_name = self.trait_name.take();
        syn::visit::visit_item_trait(self, node);
        self.owner = owner;
        self.trait_name = trait_name;
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if node.content.is_none() {
            self.facts
                .modules
                .push((node.ident.to_string(), declared_visibility(&node.vis)));
        }
        syn::visit::visit_item_mod(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let owner = impl_self_ident(node).map(ToString::to_string);
        let trait_name = node
            .trait_
            .as_ref()
            .and_then(|(_, path, _)| path.segments.last())
            .map(|segment| segment.ident.to_string());
        if let Some(owner) = &owner {
            self.facts.impls.push((owner.clone(), trait_name.clone()));
        }
        let previous_owner = std::mem::replace(&mut self.owner, owner);
        let previous_trait = std::mem::replace(&mut self.trait_name, trait_name);
        syn::visit::visit_item_impl(self, node);
        self.owner = previous_owner;
        self.trait_name = previous_trait;
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let owner = self.owner.take();
        let trait_name = self.trait_name.take();
        self.begin(None, None, &node.sig, declared_visibility(&node.vis));
        self.visit_block(&node.block);
        self.end();
        self.owner = owner;
        self.trait_name = trait_name;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.begin(
            self.owner.clone(),
            self.trait_name.clone(),
            &node.sig,
            declared_visibility(&node.vis),
        );
        self.visit_block(&node.block);
        self.end();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if let Some(block) = &node.default {
            self.begin(
                self.owner.clone(),
                None,
                &node.sig,
                DeclaredVisibility::Private,
            );
            self.visit_block(block);
            self.end();
        }
    }

    fn visit_stmt(&mut self, node: &'ast Stmt) {
        let production = match node {
            Stmt::Local(local) => self.production(&local.attrs),
            Stmt::Macro(mac) => self.production(&mac.attrs),
            Stmt::Item(item) => match ledger_owners::item_attributes(item) {
                Ok(attributes) => self.production(attributes),
                Err(_) => true,
            },
            Stmt::Expr(expr, _) => self.production(expression_attributes(expr)),
        };
        if production {
            syn::visit::visit_stmt(self, node);
        }
    }

    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        if self.production(&node.attrs) {
            syn::visit::visit_arm(self, node);
        }
    }

    fn visit_field_value(&mut self, node: &'ast syn::FieldValue) {
        if self.production(&node.attrs) {
            syn::visit::visit_field_value(self, node);
        }
    }

    fn visit_pat(&mut self, node: &'ast Pat) {
        let path = match node {
            Pat::Path(value) => Some(&value.path),
            Pat::Struct(value) => Some(&value.path),
            Pat::TupleStruct(value) => Some(&value.path),
            _ => None,
        };
        if let Some(path) = path {
            let names = path_names(path);
            if names.len() > 1 || names.first().is_some_and(|name| is_upper_name(name)) {
                let span = path
                    .segments
                    .first()
                    .map_or_else(Span::call_site, |segment| segment.ident.span());
                self.record(FunctionReference::Pattern(names), span);
            }
        }
        self.patterns += 1;
        syn::visit::visit_pat(self, node);
        self.patterns -= 1;
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if self.patterns == 0
            && let Expr::Path(function) = node.func.as_ref()
        {
            let names = path_names(&function.path);
            let span = function
                .path
                .segments
                .first()
                .map_or_else(Span::call_site, |segment| segment.ident.span());
            if let Some(label) = file_write_call(&names) {
                self.file_write(label, span);
            }
            self.record(FunctionReference::Call(names), span);
            for argument in &node.args {
                self.visit_expr(argument);
            }
            return;
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if self.patterns == 0 {
            let span = node.method.span();
            self.record(
                FunctionReference::Method(receiver_name(&node.receiver), node.method.to_string()),
                span,
            );
            if node.method == "open"
                && let Some(flags) = open_options_flags(&node.receiver)
            {
                self.file_write(format!("OpenOptions({}).open", flags.join("+")), span);
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        if self.patterns == 0 {
            let span = node
                .path
                .segments
                .first()
                .map_or_else(Span::call_site, |segment| segment.ident.span());
            self.record(FunctionReference::Struct(path_names(&node.path)), span);
        }
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if self.patterns == 0 {
            let names = path_names(&node.path);
            if names.len() > 1 || names.first().is_some_and(|name| is_upper_name(name)) {
                let span = node
                    .path
                    .segments
                    .first()
                    .map_or_else(Span::call_site, |segment| segment.ident.span());
                self.record(FunctionReference::Value(names), span);
            }
        }
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_expr_field(&mut self, node: &'ast syn::ExprField) {
        if self.patterns == 0
            && let Expr::Path(base) = node.base.as_ref()
            && base.path.is_ident("self")
            && let syn::Member::Named(field) = &node.member
        {
            self.record(
                FunctionReference::SelfField(field.to_string()),
                field.span(),
            );
        }
        syn::visit::visit_expr_field(self, node);
    }

    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        if self.patterns == 0 {
            self.record(FunctionReference::Literal(node.value()), node.span());
        }
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if self.patterns > 0 {
            return;
        }
        if node.path.is_ident("matches") {
            if let Ok((scrutinee, pattern)) = matches_parts(node) {
                self.visit_expr(&scrutinee);
                self.visit_pat(&pattern);
            }
            return;
        }
        let arguments = syn::parse::Parser::parse2(
            syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated,
            node.tokens.clone(),
        );
        match arguments {
            Ok(arguments) => {
                for argument in &arguments {
                    self.visit_expr(argument);
                }
            }
            Err(_) => {
                let mut flat = Vec::new();
                flatten_tokens(node.tokens.clone(), &mut flat);
                for (segments, span) in token_paths(&flat) {
                    if segments.len() > 1 {
                        self.record(FunctionReference::Value(segments), span);
                    }
                }
            }
        }
    }
}

/// Lists `path::Owner::function -> callee` for every production function of `sources` that
/// calls one of `callees` ([`reference_calls`]), sorted and without duplicates.
pub fn inspect_call_sites(sources: &[SourceFacts], callees: &[&str]) -> Vec<String> {
    let mut rows = sources
        .iter()
        .flat_map(|source| &source.functions)
        .flat_map(|function| {
            callees
                .iter()
                .filter(|callee| function.calls(callee))
                .map(move |callee| format!("{} -> {callee}", function.site()))
        })
        .collect::<Vec<_>>();
    rows.sort();
    rows.dedup();
    rows
}

/// Lists `path::Owner::function -> Type` for every production construction of one of `types`
/// in `sources`: a struct literal of the type or of one of its variants, a call of a path through
/// the type (`Type(..)`, `Type::Variant(..)`, `Type::new(..)`), or a variant path read as a
/// value (`Type::Unit`). Patterns are not constructions.
pub fn inspect_type_constructions(sources: &[SourceFacts], types: &[&str]) -> Vec<String> {
    let mut rows = Vec::new();
    for function in sources.iter().flat_map(|source| &source.functions) {
        for (reference, _) in &function.references {
            let path = match reference {
                FunctionReference::Struct(path)
                | FunctionReference::Call(path)
                | FunctionReference::Value(path) => path,
                _ => continue,
            };
            let named = match (reference, path.as_slice()) {
                (FunctionReference::Struct(_) | FunctionReference::Call(_), [.., last])
                    if types.contains(&last.as_str()) =>
                {
                    Some(last)
                }
                (_, [.., owner, _]) if types.contains(&owner.as_str()) => Some(owner),
                _ => None,
            };
            if let Some(named) = named {
                rows.push(format!("{} -> {named}", function.site()));
            }
        }
    }
    rows.sort();
    rows.dedup();
    rows
}

/// Whether `caller` calls `callee` by name inside one crate: a method call on `self` reaches the
/// caller's own type, a method call on another name (a local or a `self` field) every method of
/// that name (`write_all` also reaches `std::io::Write::write` implementations), and a method
/// call on a chained expression (a builder) nothing; a path call `name(..)` reaches free
/// functions, `Self::name` the caller's own type, `Type::name` that type and `module::name` free
/// functions.
fn resolves_to(caller: &FunctionFacts, callee: &FunctionFacts) -> bool {
    caller
        .references
        .iter()
        .any(|(reference, _)| match reference {
            FunctionReference::Method(receiver, method) => {
                callee.owner.is_some()
                    && receiver.is_some()
                    && (receiver.as_deref() != Some("self") || callee.owner == caller.owner)
                    && (method == &callee.name
                        || (method == "write_all"
                            && callee.name == "write"
                            && callee.trait_name.as_deref() == Some("Write")))
            }
            FunctionReference::Call(path) => {
                let Some((name, qualifier)) = path.split_last() else {
                    return false;
                };
                if name != &callee.name {
                    return false;
                }
                match qualifier.last().map(String::as_str) {
                    None => callee.owner.is_none(),
                    Some("Self") => callee.owner.is_some() && callee.owner == caller.owner,
                    Some(segment) if is_upper_name(segment) => {
                        callee.owner.as_deref() == Some(segment)
                    }
                    Some(_) => callee.owner.is_none(),
                }
            }
            _ => false,
        })
}

/// The byte sinks of one function: file creation that writes bytes (`File::create*`,
/// `fs::write`, `fs::copy`, an `OpenOptions` chain that creates, truncates or appends) and the
/// `write` of a `std::io::Write` implementation.
fn byte_sinks(function: &FunctionFacts) -> Vec<String> {
    let mut sinks = function
        .file_writes
        .iter()
        .map(|(label, _)| label)
        .filter(|label| {
            label.starts_with("File::")
                || matches!(label.as_str(), "fs::write" | "fs::copy")
                || label
                    .strip_prefix("OpenOptions(")
                    .and_then(|rest| rest.split_once(')'))
                    .is_some_and(|(flags, _)| {
                        flags.split('+').any(|flag| {
                            matches!(flag, "append" | "create" | "create_new" | "truncate")
                        })
                    })
        })
        .cloned()
        .collect::<Vec<_>>();
    if function.name == "write" && function.trait_name.as_deref() == Some("Write") {
        sinks.push("Write::write".to_string());
    }
    sinks.sort();
    sinks.dedup();
    sinks
}

/// Workflow #310 B2 (a): every byte sink ([`byte_sinks`]) of the production functions in
/// `sources` (one crate) with the capacity admission that covers it, and every named write
/// `entry` with the admission it reaches. A sink is admitted in place when its function calls
/// one of `predicates`; otherwise every chain of in-crate callers must reach a function that
/// does. Rows:
/// - `path::f -> <sinks> [admitted in place]`;
/// - `path::f -> <sinks> [admitted by A, B]`: the nearest admitting callers;
/// - `path::f -> <sinks> [NOT admitted]`: some chain of callers ends in a function that neither
///   admits nor has callers of its own;
/// - `path::entry [reaches <predicate> via a -> b]` or `path::entry [does NOT reach admission]`.
///
/// Calls resolve by name inside the crate (a method on `self` to the caller's own type), so a
/// chain is a call relation by name, not by type. An entry that names no production function,
/// or more than one, is an error.
pub fn inspect_artifact_byte_writes(
    sources: &[SourceFacts],
    predicates: &[&str],
    entries: &[&str],
) -> Result<Vec<String>, String> {
    let functions = sources
        .iter()
        .flat_map(|source| &source.functions)
        .collect::<Vec<_>>();
    if functions.is_empty() {
        return Err("no production functions to inspect for byte writes".to_string());
    }
    let admitting = |function: &FunctionFacts| {
        predicates
            .iter()
            .find(|predicate| function.calls(predicate))
            .copied()
    };
    let callers_of = |index: usize| {
        (0..functions.len())
            .filter(|caller| *caller != index && resolves_to(functions[*caller], functions[index]))
            .collect::<Vec<_>>()
    };
    let mut rows = Vec::new();
    for (index, function) in functions.iter().enumerate() {
        let sinks = byte_sinks(function);
        if sinks.is_empty() {
            continue;
        }
        let status = if admitting(function).is_some() {
            "admitted in place".to_string()
        } else {
            let mut admitted = BTreeSetString::new();
            let mut unadmitted = false;
            let mut seen = HashSet::from([index]);
            let mut queue = VecDeque::from([index]);
            while let Some(current) = queue.pop_front() {
                let callers = callers_of(current);
                if callers.is_empty() {
                    unadmitted = true;
                    continue;
                }
                for caller in callers {
                    if !seen.insert(caller) {
                        continue;
                    }
                    if admitting(functions[caller]).is_some() {
                        admitted.insert(functions[caller].qualified_name());
                    } else {
                        queue.push_back(caller);
                    }
                }
            }
            if unadmitted {
                "NOT admitted".to_string()
            } else {
                format!(
                    "admitted by {}",
                    admitted.into_iter().collect::<Vec<_>>().join(", ")
                )
            }
        };
        rows.push(format!(
            "{} -> {} [{status}]",
            function.site(),
            sinks.join(", ")
        ));
    }
    for entry in entries {
        let starts = functions
            .iter()
            .enumerate()
            .filter(|(_, function)| function.qualified_name() == *entry)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [start] = starts.as_slice() else {
            return Err(format!(
                "write entry {entry} names {} production functions",
                starts.len()
            ));
        };
        let mut previous = HashMap::new();
        let mut seen = HashSet::from([*start]);
        let mut queue = VecDeque::from([*start]);
        let mut reached = None;
        while let Some(current) = queue.pop_front() {
            if let Some(predicate) = admitting(functions[current]) {
                reached = Some((current, predicate));
                break;
            }
            for callee in 0..functions.len() {
                if callee != current
                    && resolves_to(functions[current], functions[callee])
                    && seen.insert(callee)
                {
                    previous.insert(callee, current);
                    queue.push_back(callee);
                }
            }
        }
        let status = match reached {
            Some((end, predicate)) => {
                let mut chain = vec![functions[end].qualified_name()];
                let mut current = end;
                while let Some(before) = previous.get(&current) {
                    chain.push(functions[*before].qualified_name());
                    current = *before;
                }
                chain.reverse();
                format!("reaches {predicate} via {}", chain.join(" -> "))
            }
            None => "does NOT reach admission".to_string(),
        };
        rows.push(format!("{} [{status}]", functions[*start].site()));
    }
    rows.sort();
    Ok(rows)
}

/// Workflow #310 B2 (b): every production use of a capacity admission handle in one source
/// file, with how an absent handle is decided there. A handle is `(scope, expression)`: the
/// expression (`admission`, `self.capacity`, `stream.capacity`) inside the named function
/// (`Owner::name` or `name`) or anywhere in the file (`*`), seen through `&`, `*`, parentheses,
/// `.map(..)` and the accessors `get`, `clone`, `cloned`, `copied`, `as_ref`, `as_deref`,
/// `as_mut`, `as_deref_mut`. Each row is `path::f -> <expression>: <use>`, the use being:
/// - `let-else Err(<codes>)` / `let-else no Err`, `if-let else Err(<codes>)` /
///   `if-let None falls through`, `match None Err(<codes>)` / `match None no Err`,
///   `ok_or Err(<codes>)`: an absent handle is decided here; `<codes>` are the string literals
///   passed first to a call inside the error;
/// - `None decided by .<method>` / `None decided by ?`: an absent handle turned into a value;
/// - `argument of <callee>`, `bound to <name>`, `field <name>`, `receiver of .<method>`,
///   `projection .<member>`, `other use`: the handle passed on or read, never decided.
///
/// A named handle that has no use in the file is an error.
pub fn inspect_admission_handle_uses(
    path: &str,
    source: &str,
    handles: &[(&str, &str)],
) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let mut visitor = HandleUseVisitor {
        path,
        handles,
        scope: FunctionScope::default(),
        consumed: HashSet::new(),
        rows: Vec::new(),
    };
    for item in &items {
        visitor.visit_item(item);
    }
    for (_, handle) in handles {
        if !visitor
            .rows
            .iter()
            .any(|row| row.contains(&format!(" -> {handle}: ")))
        {
            return Err(format!("{path}: admission handle {handle} has no use"));
        }
    }
    visitor.rows.sort();
    visitor.rows.dedup();
    Ok(visitor.rows)
}

struct HandleUseVisitor<'a> {
    path: &'a str,
    handles: &'a [(&'a str, &'a str)],
    scope: FunctionScope,
    consumed: HashSet<*const Expr>,
    rows: Vec<String>,
}

const HANDLE_ACCESSORS: &[&str] = &[
    "get",
    "clone",
    "cloned",
    "copied",
    "as_ref",
    "as_deref",
    "as_mut",
    "as_deref_mut",
];

const NONE_DECIDING_METHODS: &[&str] = &[
    "map_or",
    "map_or_else",
    "unwrap_or",
    "unwrap_or_else",
    "unwrap_or_default",
    "is_none",
    "is_some",
    "is_some_and",
    "is_none_or",
    "unwrap",
    "expect",
];

fn handle_text(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Path(value) => value.path.get_ident().map(ToString::to_string),
        Expr::Field(value) => {
            let base = handle_text(&value.base)?;
            match &value.member {
                syn::Member::Named(field) => Some(format!("{base}.{field}")),
                syn::Member::Unnamed(index) => Some(format!("{base}.{}", index.index)),
            }
        }
        _ => None,
    }
}

fn peel_handle(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(value) => peel_handle(&value.expr),
        Expr::Reference(value) => peel_handle(&value.expr),
        Expr::Unary(value) if matches!(value.op, syn::UnOp::Deref(_)) => peel_handle(&value.expr),
        Expr::MethodCall(call)
            if (call.args.is_empty()
                && HANDLE_ACCESSORS.contains(&call.method.to_string().as_str()))
                || (call.args.len() == 1 && call.method == "map") =>
        {
            peel_handle(&call.receiver)
        }
        other => other,
    }
}

/// `Err(<codes>)` when `visit` reaches an `Err(..)` call: its codes are the string literals
/// passed first to a call inside the error. `no Err` otherwise.
fn refusal_label(visit: impl FnOnce(&mut ErrCodeVisitor)) -> String {
    let mut visitor = ErrCodeVisitor {
        inside: 0,
        found: false,
        codes: BTreeSetString::new(),
    };
    visit(&mut visitor);
    if visitor.found {
        format!(
            "Err({})",
            visitor.codes.into_iter().collect::<Vec<_>>().join(", ")
        )
    } else {
        "no Err".to_string()
    }
}

struct ErrCodeVisitor {
    inside: usize,
    found: bool,
    codes: BTreeSetString,
}

impl ErrCodeVisitor {
    fn first_literal(&mut self, arguments: &syn::punctuated::Punctuated<Expr, syn::Token![,]>) {
        if self.inside > 0
            && let Some(Expr::Lit(syn::ExprLit {
                lit: Lit::Str(code),
                ..
            })) = arguments.first()
        {
            self.codes.insert(code.value());
        }
    }
}

impl<'ast> Visit<'ast> for ErrCodeVisitor {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        let is_err =
            matches!(node.func.as_ref(), Expr::Path(function) if function.path.is_ident("Err"));
        if is_err {
            self.found = true;
            self.inside += 1;
        }
        self.first_literal(&node.args);
        syn::visit::visit_expr_call(self, node);
        if is_err {
            self.inside -= 1;
        }
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.first_literal(&node.args);
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// The codes of an `ok_or(..)` / `ok_or_else(..)` error value: the string literals passed first
/// to a call inside it.
fn error_value_label(arguments: &syn::punctuated::Punctuated<Expr, syn::Token![,]>) -> String {
    let mut visitor = ErrCodeVisitor {
        inside: 1,
        found: true,
        codes: BTreeSetString::new(),
    };
    for argument in arguments {
        visitor.visit_expr(argument);
    }
    format!(
        "Err({})",
        visitor.codes.into_iter().collect::<Vec<_>>().join(", ")
    )
}

impl HandleUseVisitor<'_> {
    fn handle_of<'e>(&self, expr: &'e Expr) -> Option<(String, &'e Expr)> {
        let peeled = peel_handle(expr);
        let text = handle_text(peeled)?;
        let scope = self.scope.name();
        self.handles
            .iter()
            .any(|(handle_scope, handle)| {
                *handle == text && (*handle_scope == "*" || *handle_scope == scope)
            })
            .then_some((text, peeled))
    }

    fn record(&mut self, handle: &str, peeled: &Expr, use_label: String) {
        self.consumed.insert(peeled as *const Expr);
        self.rows.push(format!(
            "{}::{} -> {handle}: {use_label}",
            self.path,
            self.scope.name()
        ));
    }

    fn argument_uses(
        &mut self,
        callee: &str,
        arguments: &syn::punctuated::Punctuated<Expr, syn::Token![,]>,
    ) {
        for argument in arguments {
            if let Some((handle, peeled)) = self.handle_of(argument) {
                self.record(&handle, peeled, format!("argument of {callee}"));
            }
        }
    }

    fn let_scrutinees<'e>(condition: &'e Expr, lets: &mut Vec<&'e syn::ExprLet>) {
        match condition {
            Expr::Let(value) => lets.push(value),
            Expr::Binary(value) if matches!(value.op, BinOp::And(_)) => {
                Self::let_scrutinees(&value.left, lets);
                Self::let_scrutinees(&value.right, lets);
            }
            Expr::Paren(value) => Self::let_scrutinees(&value.expr, lets),
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for HandleUseVisitor<'_> {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let owner = impl_self_ident(node).map(ToString::to_string);
        let previous = std::mem::replace(&mut self.scope.owner, owner);
        syn::visit::visit_item_impl(self, node);
        self.scope.owner = previous;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let previous = self.scope.function.replace(node.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, node);
        self.scope.function = previous;
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let owner = self.scope.owner.take();
        let previous = self.scope.function.replace(node.sig.ident.to_string());
        syn::visit::visit_item_fn(self, node);
        self.scope.function = previous;
        self.scope.owner = owner;
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init
            && let Some((handle, peeled)) = self.handle_of(&init.expr)
        {
            let use_label = match &init.diverge {
                Some((_, otherwise)) => format!(
                    "let-else {}",
                    refusal_label(|visitor| visitor.visit_expr(otherwise))
                ),
                None => format!(
                    "bound to {}",
                    local_binding(&node.pat).unwrap_or_else(|| "a pattern".to_string())
                ),
            };
            self.record(&handle, peeled, use_label);
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        let mut lets = Vec::new();
        Self::let_scrutinees(&node.cond, &mut lets);
        for scrutinee in lets {
            if let Some((handle, peeled)) = self.handle_of(&scrutinee.expr) {
                let use_label = match &node.else_branch {
                    Some((_, otherwise)) => format!(
                        "if-let else {}",
                        refusal_label(|visitor| visitor.visit_expr(otherwise))
                    ),
                    None => "if-let None falls through".to_string(),
                };
                self.record(&handle, peeled, use_label);
            }
        }
        syn::visit::visit_expr_if(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        if let Some((handle, peeled)) = self.handle_of(&node.expr) {
            let absent = node.arms.iter().find(|arm| {
                matches!(&arm.pat, Pat::Ident(value) if value.ident == "None")
                    || matches!(&arm.pat, Pat::Wild(_))
                    || matches!(&arm.pat, Pat::Path(value) if value.path.is_ident("None"))
            });
            let use_label = match absent {
                Some(arm) => format!(
                    "match None {}",
                    refusal_label(|visitor| visitor.visit_expr(&arm.body))
                ),
                None => "match without a None arm".to_string(),
            };
            self.record(&handle, peeled, use_label);
        }
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        if let Some((handle, peeled)) = self.handle_of(&node.expr) {
            self.record(&handle, peeled, "None decided by ?".to_string());
        }
        syn::visit::visit_expr_try(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if !self
            .consumed
            .contains(&(node.receiver.as_ref() as *const Expr))
            && let Some((handle, peeled)) = self.handle_of(&node.receiver)
            && !self.consumed.contains(&(peeled as *const Expr))
        {
            let use_label = if matches!(method.as_str(), "ok_or" | "ok_or_else") {
                format!("ok_or {}", error_value_label(&node.args))
            } else if NONE_DECIDING_METHODS.contains(&method.as_str()) {
                format!("None decided by .{method}")
            } else {
                format!("receiver of .{method}")
            };
            self.record(&handle, peeled, use_label);
        }
        self.argument_uses(&format!(".{method}"), &node.args);
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        let callee = match node.func.as_ref() {
            Expr::Path(function) => path_names(&function.path).join("::"),
            _ => "<expression>".to_string(),
        };
        self.argument_uses(&callee, &node.args);
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_field_value(&mut self, node: &'ast syn::FieldValue) {
        if let Some((handle, peeled)) = self.handle_of(&node.expr) {
            let field = match &node.member {
                syn::Member::Named(field) => field.to_string(),
                syn::Member::Unnamed(index) => index.index.to_string(),
            };
            self.record(&handle, peeled, format!("field {field}"));
        }
        syn::visit::visit_field_value(self, node);
    }

    fn visit_expr_field(&mut self, node: &'ast syn::ExprField) {
        if !self.consumed.contains(&(node.base.as_ref() as *const Expr))
            && let Some(text) = handle_text(&node.base)
            && let Some((handle, peeled)) = self.handle_of(&node.base)
            && text == handle
        {
            let member = match &node.member {
                syn::Member::Named(field) => field.to_string(),
                syn::Member::Unnamed(index) => index.index.to_string(),
            };
            self.record(&handle, peeled, format!("projection .{member}"));
        }
        syn::visit::visit_expr_field(self, node);
    }

    fn visit_expr(&mut self, node: &'ast Expr) {
        if matches!(node, Expr::Field(_) | Expr::Path(_))
            && !self.consumed.contains(&(node as *const Expr))
            && let Some(text) = handle_text(node)
            && let Some((handle, peeled)) = self.handle_of(node)
            && text == handle
        {
            self.record(&handle, peeled, "other use".to_string());
        }
        syn::visit::visit_expr(self, node);
    }
}

/// Workflow #310 B4: every production call in one source file that handles a document
/// envelope, as `path::Owner::function -> <callee>(<arguments>)`. `api` names the calls to
/// report: `Type::name` or `name` for a call through a path ending so, `.method` for a method
/// call, `.method@Type` for a method call inside a function whose signature names `Type`.
/// A call is also reported when one of its arguments is a variant of one of `kind_types`. Each
/// argument is rendered as `Kind::Variant` (a `kind_types` variant), `"text"` (a string
/// literal) or `_`. Macro bodies are read when they parse as comma-separated expressions.
pub fn inspect_envelope_sites(
    path: &str,
    source: &str,
    api: &[&str],
    kind_types: &[&str],
) -> Result<Vec<String>, String> {
    struct EnvelopeVisitor<'a> {
        path: &'a str,
        api: &'a [&'a str],
        kind_types: &'a [&'a str],
        scope: FunctionScope,
        signature_types: Vec<BTreeSetString>,
        rows: Vec<String>,
    }
    impl EnvelopeVisitor<'_> {
        fn render(&self, argument: &Expr) -> (String, bool) {
            let mut argument = argument;
            while let Expr::Reference(value) = argument {
                argument = &value.expr;
            }
            match argument {
                Expr::Path(value) => {
                    let names = path_names(&value.path);
                    match names.as_slice() {
                        [.., kind, variant] if self.kind_types.contains(&kind.as_str()) => {
                            (format!("{kind}::{variant}"), true)
                        }
                        _ => ("_".to_string(), false),
                    }
                }
                Expr::Lit(syn::ExprLit {
                    lit: Lit::Str(text),
                    ..
                }) => (format!("{:?}", text.value()), false),
                _ => ("_".to_string(), false),
            }
        }

        fn report(
            &mut self,
            callee: String,
            named: bool,
            arguments: &syn::punctuated::Punctuated<Expr, syn::Token![,]>,
        ) {
            let rendered = arguments
                .iter()
                .map(|argument| self.render(argument))
                .collect::<Vec<_>>();
            if named || rendered.iter().any(|(_, kind)| *kind) {
                self.rows.push(format!(
                    "{}::{} -> {callee}({})",
                    self.path,
                    self.scope.name(),
                    rendered
                        .into_iter()
                        .map(|(text, _)| text)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        fn enter(&mut self, signature: &syn::Signature) {
            struct TypeNames<'a>(&'a mut BTreeSetString);
            impl<'ast> Visit<'ast> for TypeNames<'_> {
                fn visit_path_segment(&mut self, segment: &'ast syn::PathSegment) {
                    self.0.insert(segment.ident.to_string());
                    syn::visit::visit_path_segment(self, segment);
                }
            }
            let mut names = BTreeSetString::new();
            TypeNames(&mut names).visit_signature(signature);
            self.signature_types.push(names);
        }
    }
    impl<'ast> Visit<'ast> for EnvelopeVisitor<'_> {
        fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
            let owner = impl_self_ident(node).map(ToString::to_string);
            let previous = std::mem::replace(&mut self.scope.owner, owner);
            syn::visit::visit_item_impl(self, node);
            self.scope.owner = previous;
        }

        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            let previous = self.scope.function.replace(node.sig.ident.to_string());
            self.enter(&node.sig);
            syn::visit::visit_impl_item_fn(self, node);
            self.signature_types.pop();
            self.scope.function = previous;
        }

        fn visit_item_fn(&mut self, node: &'ast ItemFn) {
            let owner = self.scope.owner.take();
            let previous = self.scope.function.replace(node.sig.ident.to_string());
            self.enter(&node.sig);
            syn::visit::visit_item_fn(self, node);
            self.signature_types.pop();
            self.scope.function = previous;
            self.scope.owner = owner;
        }

        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let Expr::Path(function) = node.func.as_ref() {
                let names = path_names(&function.path);
                let named = self.api.iter().any(|spec| {
                    !spec.starts_with('.')
                        && path_ends_with(&names, &spec.split("::").collect::<Vec<_>>())
                });
                self.report(names.join("::"), named, &node.args);
            }
            syn::visit::visit_expr_call(self, node);
        }

        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            let method = node.method.to_string();
            let named = self.api.iter().any(|spec| {
                let Some(spec) = spec.strip_prefix('.') else {
                    return false;
                };
                match spec.split_once('@') {
                    Some((name, required)) => {
                        name == method
                            && self
                                .signature_types
                                .last()
                                .is_some_and(|types| types.contains(required))
                    }
                    None => spec == method,
                }
            });
            self.report(format!(".{method}"), named, &node.args);
            syn::visit::visit_expr_method_call(self, node);
        }

        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            if let Ok(arguments) = syn::parse::Parser::parse2(
                syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated,
                node.tokens.clone(),
            ) {
                for argument in &arguments {
                    self.visit_expr(argument);
                }
            }
        }
    }

    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    if !ledger_owners::production_attributes(&file.attrs)? {
        return Ok(Vec::new());
    }
    let items = ledger_owners::production_items(&file.items)?;
    let mut visitor = EnvelopeVisitor {
        path,
        api,
        kind_types,
        scope: FunctionScope::default(),
        signature_types: Vec::new(),
        rows: Vec::new(),
    };
    for item in &items {
        visitor.visit_item(item);
    }
    visitor.rows.sort();
    visitor.rows.dedup();
    Ok(visitor.rows)
}

/// The exact source text of the one production function `owner::function` (`owner` `None` for a
/// free function) in one file, from its `fn` keyword to its closing brace.
pub fn function_source(
    path: &str,
    source: &str,
    owner: Option<&str>,
    function: &str,
) -> Result<String, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = ledger_owners::production_items(&file.items)?;
    let mut all = Vec::new();
    collect_nested_items(&items, &mut all);
    let mut spans = Vec::new();
    for item in all {
        match (owner, item) {
            (None, Item::Fn(item_fn)) if item_fn.sig.ident == function => {
                spans.push((
                    item_fn.sig.fn_token.span,
                    item_fn.block.brace_token.span.close(),
                ));
            }
            (Some(owner), Item::Impl(item_impl))
                if impl_self_ident(item_impl).is_some_and(|ident| ident == owner) =>
            {
                for member in &item_impl.items {
                    if let syn::ImplItem::Fn(method) = member
                        && method.sig.ident == function
                    {
                        spans.push((
                            method.sig.fn_token.span,
                            method.block.brace_token.span.close(),
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    let [(start, end)] = spans.as_slice() else {
        return Err(format!(
            "{path}: expected one production function {}{function}, found {}",
            owner.map(|owner| format!("{owner}::")).unwrap_or_default(),
            spans.len()
        ));
    };
    let (start, end) = (start.start(), end.end());
    let lines = source.lines().collect::<Vec<_>>();
    if start.line == 0 || end.line > lines.len() || start.line > end.line {
        return Err(format!("{path}: {function} has no source location"));
    }
    let mut text = String::new();
    for (index, line) in lines[start.line - 1..end.line].iter().enumerate() {
        let number = start.line + index;
        let from = if number == start.line {
            line.char_indices()
                .nth(start.column)
                .map_or(line.len(), |(offset, _)| offset)
        } else {
            0
        };
        let to = if number == end.line {
            line.char_indices()
                .nth(end.column)
                .map_or(line.len(), |(offset, _)| offset)
        } else {
            line.len()
        };
        text.push_str(&line[from..to]);
        if number != end.line {
            text.push('\n');
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn generic_runtime_guard_rejects_project_specific_branch() {
        let source = r#"
            fn select(game: &str) -> bool {
                game == "arknights"
            }
        "#;

        let violations =
            super::inspect_generic_runtime_identity("crates/runtime/src/lib.rs", source);

        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("arknights"));
    }

    #[test]
    fn generic_runtime_guard_enforces_compound_token_boundaries_without_generic_false_positives() {
        let forbidden = r#"
            const POLICY_BA_MODE: &str = "fixture";
            struct ArknightsCompiler;
        "#;
        let allowed = r#"
            const SERVER_BASE: &str = "neutral";
            const SERVER_BACKUP: &str = "neutral";
            const SERVER_BALANCE: &str = "neutral";
            const OCR_LANGUAGE: &str = "zh_cn";
            fn exercise_plan() {}
            fn recruit_worker() {}
            fn sanity_check() {}
            fn backend_banner() {}
        "#;

        let violations = super::inspect_generic_runtime_identity("fixture.rs", forbidden);
        assert_eq!(violations.len(), 2);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("project-specific word ba"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("project-specific word arknights"))
        );
        assert!(super::inspect_generic_runtime_identity("fixture.rs", allowed).is_empty());
        let server = super::inspect_generic_runtime_identity(
            "fixture.rs",
            "const SERVER_JP: &str = \"neutral\";",
        );
        assert_eq!(server.len(), 1);
        assert!(server[0].contains("project-specific token server_jp"));
        let standalone =
            super::inspect_generic_runtime_identity("fixture.rs", "const LOCALE: &str = \"jp\";");
        assert_eq!(standalone.len(), 1);
        assert!(standalone[0].contains("project-specific word jp"));
    }

    #[test]
    fn identifier_words_cover_common_rust_identifier_styles_and_acronyms() {
        assert_eq!(
            super::identifier_words("policy_ba_mode POLICY_BA_MODE policyBaMode PolicyBaMode"),
            [
                "policy", "ba", "mode", "policy", "ba", "mode", "policy", "ba", "mode", "policy",
                "ba", "mode"
            ]
        );
        assert_eq!(
            super::identifier_words("HTTPServerBAConfig"),
            ["http", "server", "ba", "config"]
        );
        for identifier in [
            "ARKNIGHTSCOMPILER",
            "aRkNiGhTsCompiler",
            "BLUEARCHIVEHOME",
            "bA",
        ] {
            let source = format!("struct {identifier};");
            assert!(
                !super::inspect_generic_runtime_identity("fixture.rs", &source).is_empty(),
                "{identifier}"
            );
            assert!(
                !super::inspect_generic_authoring_identity("fixture.rs", &source)
                    .unwrap()
                    .is_empty(),
                "{identifier}"
            );
        }
        for identifier in ["SERVERCN", "sErVeRjP"] {
            assert!(
                !super::inspect_generic_runtime_identity("fixture.rs", identifier).is_empty(),
                "{identifier}"
            );
        }
        let allowed = r#"
            const SERVER_BASE: &str = "neutral";
            const BACKUP: &str = "neutral";
            const BALANCE: &str = "neutral";
            const AZURE: &str = "neutral";
            const HASH: &str = "f0a5b8536ac19f3df43dcae823c13466ad4d3a13";
            const LANGUAGE: &str = "zh_cn";
            fn backend_banner() {}
        "#;
        assert!(super::inspect_generic_runtime_identity("fixture.rs", allowed).is_empty());
        assert!(
            super::inspect_generic_authoring_identity("fixture.rs", allowed)
                .unwrap()
                .is_empty()
        );
        let provider = "external-tools/vision/fastdeploy/fastdeploy_ppocr_maa.dll";
        assert!(
            super::inspect_generic_runtime_identity("crates/vision-ffi/src/lib.rs", provider)
                .is_empty()
        );
        assert!(
            !super::inspect_generic_runtime_identity("crates/runtime-host/src/lib.rs", provider)
                .is_empty()
        );
    }

    #[test]
    fn generic_authoring_guard_rejects_production_identity_and_skips_tests() {
        let forbidden = r#"fn select() -> &'static str { "arknights.cn" }"#;
        let test_only = r#"
            #[cfg(test)]
            mod tests {
                const GAME: &str = "arknights";
            }
        "#;

        assert_eq!(
            super::inspect_generic_authoring_identity("fixture.rs", forbidden)
                .unwrap()
                .len(),
            1
        );
        assert!(
            super::inspect_generic_authoring_identity("fixture.rs", test_only)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn generic_authoring_guard_rejects_compound_identifiers_and_byte_strings() {
        let forbidden = r#"
            fn compile_arknights_graph() {}
            struct ArknightsCompiler;
            const POLICY_BA_MODE: bool = true;
            const BLUEARCHIVE_HOME: &[u8] = b"arknights";
        "#;
        let allowed = r#"
            fn backend_banner() {}
            fn exercise_plan() {}
            fn recruit_worker() {}
            fn sanity_check() {}
            const SERVER_BASE: &str = "neutral";
            const SERVER_BACKUP: &str = "neutral";
            const SERVER_BALANCE: &str = "neutral";
            const SERVER_BANNER: &[u8] = b"neutral";
        "#;

        let violations =
            super::inspect_generic_authoring_identity("fixture.rs", forbidden).unwrap();
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("compile_arknights_graph"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("ArknightsCompiler")),
            "{violations:#?}"
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("POLICY_BA_MODE"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("BLUEARCHIVE_HOME"))
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("byte string"))
        );
        assert!(
            super::inspect_generic_authoring_identity("fixture.rs", allowed)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn global_append_guard_rejects_any_ingress_other_than_sanitized_event_draft() {
        let forbidden = r#"
            pub struct GlobalLedger;
            pub struct EventDraft;
            pub struct SanitizedEventDraft;
            impl GlobalLedger {
                pub fn append(&self, draft: SanitizedEventDraft) { let _ = draft; }
                pub fn append_raw(&self, draft: EventDraft) { let _ = draft; }
            }
        "#;
        let allowed = r#"
            pub struct GlobalLedger;
            pub struct SanitizedEventDraft;
            impl GlobalLedger {
                pub fn append(&self, draft: SanitizedEventDraft) { let _ = draft; }
            }
        "#;

        let violations = super::inspect_global_append_ingress("fixture.rs", forbidden).unwrap();
        assert!(
            violations
                .iter()
                .any(|item| item.contains("alternate public event ingress append_raw"))
        );
        assert!(
            super::inspect_global_append_ingress("fixture.rs", allowed)
                .unwrap()
                .is_empty()
        );
        let joint = r#"
            impl GlobalLedger {
                pub fn append(&self, draft: SanitizedEventDraft) {}
                pub fn append_transaction(&self, draft: SanitizedEventDraft, work: Box<dyn LedgerTransactionWork>) {}
                pub fn append_deferred(&self, draft: SanitizedEventDraft) {}
            }
        "#;
        assert!(
            super::inspect_global_append_ingress("fixture.rs", joint)
                .unwrap()
                .is_empty()
        );
        for invalid in [
            joint.replace("draft: SanitizedEventDraft,", "draft: serde_json::Value,"),
            joint.replace("Box<dyn LedgerTransactionWork>", "Box<dyn Fn()>"),
            joint.replace(
                "append_deferred(&self, draft: SanitizedEventDraft)",
                "append_deferred(&self, draft: serde_json::Value)",
            ),
        ] {
            assert!(
                !super::inspect_global_append_ingress("fixture.rs", &invalid)
                    .unwrap()
                    .is_empty()
            );
        }
        let scoped = |child: &str| {
            let root = r#"
                use serde_json::Value as Payload;
                pub struct GlobalLedger;
                pub struct PersistedEvent { sequence: u64 }
                pub struct LedgerRecord { pub payload: Payload }
                pub struct SanitizedEventDraft;
                type Owner = GlobalLedger;
            "#;
            let mut owners = [
                ("crate", root),
                ("crate::writer", child),
                ("crate::sibling", "type Owner = Unrelated;"),
            ]
            .into_iter()
            .map(|(module, source)| {
                let file = syn::parse_file(source).unwrap();
                let items = super::ledger_owners::production_items(&file.items).unwrap();
                super::LedgerOwnerModule {
                    path: format!("{module}.rs").into(),
                    module: module.to_string(),
                    aliases: super::local_type_aliases(&items),
                    items,
                }
            })
            .collect::<Vec<_>>();
            super::ledger_owners::resolve_module_aliases(&mut owners).unwrap();
            owners
        };
        let writer = r#"
            use super::{Owner as Writer, SanitizedEventDraft as Clean};
            impl Writer {
                pub fn append(&self, draft: Clean) {}
                #[cfg(all(test, feature = "fixture"))]
                pub fn append_test(&self, draft: Raw) {}
            }
        "#;
        let owners = scoped(writer);
        assert!(
            super::inspect_ledger_append_ingress(&owners)
                .unwrap()
                .is_empty()
        );
        assert!(
            super::inspect_ledger_public_api(&owners)
                .unwrap()
                .is_empty()
        );
        assert!(
            super::inspect_ledger_forbidden_sources(&owners)
                .unwrap()
                .is_empty()
        );
        let duplicate = scoped(&format!(
            "{writer} impl Writer {{ pub fn append(&self, draft: Clean) {{}} }}"
        ));
        assert!(
            super::inspect_ledger_append_ingress(&duplicate)
                .unwrap()
                .iter()
                .any(|error| error.contains("found 2"))
        );
        let raw = scoped(&writer.replace("draft: Clean", "draft: serde_json::Value"));
        assert!(
            !super::inspect_ledger_append_ingress(&raw)
                .unwrap()
                .is_empty()
        );
        let leak = scoped(&format!(
            r#"{writer}
            use super::{{Payload, PersistedEvent}};
            #[cfg(any(test, feature = "production"))]
            impl PersistedEvent {{ pub fn leaked(&self) -> Payload {{ todo!() }} }}
        "#
        ));
        assert!(!super::inspect_ledger_public_api(&leak).unwrap().is_empty());
        let forbidden = scoped(&format!(
            "{writer} fn active_tests_name() {{ std::panic::catch_unwind(|| ()); }}"
        ));
        assert!(
            !super::inspect_ledger_forbidden_sources(&forbidden)
                .unwrap()
                .is_empty()
        );
        let test_only = scoped(&format!(
            "{writer} #[cfg(test)] fn excluded() {{ std::panic::catch_unwind(|| ()); }}"
        ));
        assert!(
            super::inspect_ledger_forbidden_sources(&test_only)
                .unwrap()
                .is_empty()
        );
        let test_block = scoped(&format!(
            "{writer} fn production() {{ #[cfg(test)] {{ std::panic::catch_unwind(|| ()); }} }}"
        ));
        assert!(
            super::inspect_ledger_forbidden_sources(&test_block)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn persisted_fact_guard_rejects_deserialize_public_fields_and_public_constructors() {
        let forbidden = r#"
            use serde::Deserialize;
            #[derive(Deserialize)]
            pub struct PersistedEvent { pub sequence: u64 }
            impl PersistedEvent {
                pub fn new(sequence: u64) -> Self { Self { sequence } }
            }
        "#;
        let allowed = r#"
            #[derive(Clone)]
            pub struct PersistedEvent { sequence: u64 }
            impl PersistedEvent {
                pub(crate) fn from_sanitized(sequence: u64) -> Self { Self { sequence } }
                pub fn sequence(&self) -> u64 { self.sequence }
            }
        "#;

        let violations = super::inspect_persisted_event_ownership("fixture.rs", forbidden).unwrap();
        assert!(violations.iter().any(|item| item.contains("Deserialize")));
        assert!(violations.iter().any(|item| item.contains("public field")));
        assert!(
            violations
                .iter()
                .any(|item| item.contains("public constructor"))
        );
        assert!(
            super::inspect_persisted_event_ownership("fixture.rs", allowed)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn ledger_matching_guard_rejects_contract_owned_fact_matching() {
        let forbidden = r#"
            pub struct EventQuery;
            pub struct PersistedEvent;
            impl EventQuery {
                pub fn matches(&self, event: &PersistedEvent) -> bool { let _ = event; true }
            }
        "#;
        let allowed_contract = "pub struct EventQuery;";
        let allowed_ledger = r#"
            fn query_matches(query: &EventQuery, event: &PersistedEvent) -> bool {
                let _ = (query, event);
                true
            }
        "#;

        assert!(
            !super::inspect_contract_fact_matching("fixture.rs", forbidden)
                .unwrap()
                .is_empty()
        );
        assert!(
            super::inspect_contract_fact_matching("fixture.rs", allowed_contract)
                .unwrap()
                .is_empty()
        );
        assert!(super::ledger_owns_query_matching("fixture.rs", allowed_ledger).unwrap());
    }

    #[test]
    fn producer_capability_guard_rejects_transport_ids_and_raw_artifacts() {
        let forbidden = r#"
            pub struct EventDraft;
            impl EventDraft {
                pub fn new(event_id: EventId, links: EventLinks) -> Self { let _ = (event_id, links); Self }
                pub fn with_artifacts(self, artifacts: Vec<ArtifactReference>) -> Self { let _ = artifacts; self }
            }
            pub struct EventLinksDraft;
            impl EventLinksDraft {
                pub fn with_request_id(self, value: RequestId) -> Self { let _ = value; self }
            }
        "#;
        let allowed = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub struct EventDraft;
            impl EventDraft {
                pub fn new(event_id: IssuedEventId, links: EventLinksDraft) -> Self { let _ = (event_id, links); Self }
                pub fn with_artifacts(self, artifacts: Vec<StoreIssuedArtifact>) -> Self { let _ = artifacts; self }
            }
            pub struct EventLinksDraft;
            impl EventLinksDraft {
                pub fn with_instance_id(self, value: IssuedInstanceId) -> Self { let _ = value; self }
                pub fn with_request_id(self, value: IssuedRequestId) -> Self { let _ = value; self }
                pub fn with_correlation_id(self, value: IssuedCorrelationId) -> Self { let _ = value; self }
                pub fn with_causation_id(self, value: IssuedCausationId) -> Self { let _ = value; self }
                pub fn with_task_id(self, value: IssuedTaskId) -> Self { let _ = value; self }
                pub fn with_run_id(self, value: IssuedRunId) -> Self { let _ = value; self }
                pub fn with_lease_id(self, value: IssuedLeaseId) -> Self { let _ = value; self }
                pub fn with_frame_id(self, value: IssuedFrameId) -> Self { let _ = value; self }
                pub fn with_action_id(self, value: IssuedActionId) -> Self { let _ = value; self }
                pub fn with_recognition_id(self, value: IssuedRecognitionId) -> Self { let _ = value; self }
            }
        "#;

        assert!(
            !super::inspect_producer_event_capabilities("fixture.rs", forbidden)
                .unwrap()
                .is_empty()
        );
        assert!(
            super::inspect_producer_event_capabilities("fixture.rs", allowed)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn producer_capability_guard_rejects_public_artifact_authority_and_undefined_capabilities() {
        let public_issuer = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub struct ArtifactStoreIssuer;
            impl ArtifactStoreIssuer {
                pub fn new() -> Self { Self }
                pub fn issue_pending(&self) -> StoreIssuedArtifact { StoreIssuedArtifact { reference: 1 } }
            }
        "#;
        let undefined_capability = r#"
            pub struct EventDraft;
            impl EventDraft {
                pub fn new(event_id: IssuedEventId, links: EventLinksDraft) -> Self { let _ = (event_id, links); Self }
                pub fn with_artifacts(self, artifacts: Vec<StoreIssuedArtifact>) -> Self { let _ = artifacts; self }
            }
            pub struct EventLinksDraft;
            impl EventLinksDraft {
                pub fn with_instance_id(self, value: IssuedInstanceId) -> Self { let _ = value; self }
                pub fn with_request_id(self, value: IssuedRequestId) -> Self { let _ = value; self }
                pub fn with_correlation_id(self, value: IssuedCorrelationId) -> Self { let _ = value; self }
                pub fn with_causation_id(self, value: IssuedCausationId) -> Self { let _ = value; self }
                pub fn with_task_id(self, value: IssuedTaskId) -> Self { let _ = value; self }
                pub fn with_run_id(self, value: IssuedRunId) -> Self { let _ = value; self }
                pub fn with_lease_id(self, value: IssuedLeaseId) -> Self { let _ = value; self }
                pub fn with_frame_id(self, value: IssuedFrameId) -> Self { let _ = value; self }
                pub fn with_action_id(self, value: IssuedActionId) -> Self { let _ = value; self }
                pub fn with_recognition_id(self, value: IssuedRecognitionId) -> Self { let _ = value; self }
            }
        "#;
        let public_free_ingress = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub fn issue_pending() -> StoreIssuedArtifact { StoreIssuedArtifact { reference: 1 } }
        "#;
        let public_trait_ingress = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub trait ArtifactIngress {
                fn issue_pending(&self) -> StoreIssuedArtifact;
            }
        "#;
        let renamed_inherent_ingress = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub struct RenamedStoreBoundary;
            impl RenamedStoreBoundary {
                pub fn issue_pending(&self) -> StoreIssuedArtifact {
                    StoreIssuedArtifact { reference: 1 }
                }
            }
        "#;
        let receiver_promotion = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub struct ArtifactReference;
            impl ArtifactReference {
                pub fn promote(self) -> StoreIssuedArtifact {
                    StoreIssuedArtifact { reference: 1 }
                }
            }
        "#;
        let conversion_ingress = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub struct ArtifactReference;
            impl From<ArtifactReference> for StoreIssuedArtifact {
                fn from(_: ArtifactReference) -> Self {
                    Self { reference: 1 }
                }
            }
        "#;
        let nested_module_ingress = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            mod newly_added_event_module {
                pub fn issue_pending() -> super::StoreIssuedArtifact {
                    super::StoreIssuedArtifact { reference: 1 }
                }
            }
        "#;
        let aliased_method_ingress = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            type Attachment = StoreIssuedArtifact;
            pub struct Boundary;
            impl Boundary {
                pub fn issue_pending(&self) -> Attachment {
                    StoreIssuedArtifact { reference: 1 }
                }
            }
        "#;
        let wrapped_alias_ingress = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            type Attachment = Result<StoreIssuedArtifact, ()>;
            pub fn issue_pending() -> Attachment {
                Ok(StoreIssuedArtifact { reference: 1 })
            }
        "#;
        let approved_store_issuer = r#"
            pub struct StoreIssuedArtifact { reference: u64 }
            pub struct ArtifactStoreIssuer { identifiers: u64 }
            pub struct ArtifactKind;
            pub struct ArtifactLinksDraft;
            pub struct ArtifactIssuePolicy;
            pub struct SanitizationError;
            impl ArtifactStoreIssuer {
                pub fn issue(
                    &self,
                    kind: ArtifactKind,
                    links: ArtifactLinksDraft,
                    bytes: &[u8],
                    created_at_unix_ms: u64,
                    policy: ArtifactIssuePolicy,
                ) -> Result<StoreIssuedArtifact, SanitizationError> {
                    let _ = (self, kind, links, bytes, created_at_unix_ms, policy);
                    Ok(StoreIssuedArtifact { reference: 1 })
                }
            }
        "#;

        let issuer_violations =
            super::inspect_producer_event_capabilities("fixture.rs", public_issuer).unwrap();
        assert!(
            !issuer_violations.is_empty(),
            "public artifact issuance authority must be rejected"
        );

        let undefined_violations =
            super::inspect_producer_event_capabilities("fixture.rs", undefined_capability).unwrap();
        assert!(
            !undefined_violations.is_empty(),
            "undefined StoreIssuedArtifact capability must be rejected"
        );
        assert!(
            !super::inspect_producer_event_capabilities("fixture.rs", public_free_ingress)
                .unwrap()
                .is_empty(),
            "public free ingress to StoreIssuedArtifact must be rejected"
        );
        assert!(
            !super::inspect_producer_event_capabilities("fixture.rs", public_trait_ingress)
                .unwrap()
                .is_empty(),
            "public trait ingress to StoreIssuedArtifact must be rejected"
        );
        for (label, source) in [
            ("renamed inherent issuer", renamed_inherent_ingress),
            ("receiver promotion", receiver_promotion),
            ("conversion implementation", conversion_ingress),
            ("new nested event module", nested_module_ingress),
            ("aliased method return", aliased_method_ingress),
            ("wrapped alias return", wrapped_alias_ingress),
        ] {
            assert!(
                !super::inspect_producer_event_capabilities("fixture.rs", source)
                    .unwrap()
                    .is_empty(),
                "{label} must be rejected"
            );
        }
        assert!(
            super::inspect_producer_event_capabilities("fixture.rs", approved_store_issuer)
                .unwrap()
                .is_empty(),
            "the C2 artifact-store issuer boundary must remain allowed"
        );
    }

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
    fn resource_tooling_guard_rejects_direct_and_transitive_production_dependencies() {
        let metadata = serde_json::json!({
            "packages": [
                {"id": "tooling", "name": "actingcommand-resource-tooling"},
                {"id": "lab", "name": "actingcommand-lab"},
                {"id": "lab-cli", "name": "actingcommand-actinglab"},
                {"id": "bridge", "name": "runtime-bridge"},
                {"id": "transitive", "name": "runtime-transitive"},
                {"id": "clean", "name": "runtime-clean"}
            ],
            "workspace_members": ["tooling", "lab", "lab-cli", "bridge", "transitive", "clean"],
            "resolve": {
                "nodes": [
                    {"id": "tooling", "dependencies": []},
                    {"id": "lab", "dependencies": ["tooling"]},
                    {"id": "lab-cli", "dependencies": ["tooling"]},
                    {"id": "bridge", "dependencies": ["tooling"]},
                    {"id": "transitive", "dependencies": ["bridge"]},
                    {"id": "clean", "dependencies": []}
                ]
            }
        });

        let violations = super::resource_tooling_removability_violations(
            &metadata.to_string(),
            &[
                "actingcommand-resource-tooling",
                "actingcommand-lab",
                "actingcommand-actinglab",
            ],
        )
        .unwrap();

        assert_eq!(
            violations,
            vec![
                "production package runtime-bridge reaches actingcommand-resource-tooling: runtime-bridge -> actingcommand-resource-tooling",
                "production package runtime-transitive reaches actingcommand-resource-tooling: runtime-transitive -> runtime-bridge -> actingcommand-resource-tooling"
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
            pub struct Event { pub payload: JsonValue, private: JsonValue }
            pub enum Projection { Full(JsonValue), Omitted }
            fn private_helper() -> JsonValue { unreachable!() }
        "#;

        let violations = super::inspect_public_api("fixture.rs", source).unwrap();

        assert!(violations.iter().any(|item| item.contains("direct")));
        assert!(violations.iter().any(|item| item.contains("aliased")));
        assert!(violations.iter().any(|item| item.contains("module_alias")));
        assert!(violations.iter().any(|item| item.contains("Port::carry")));
        assert!(violations.iter().any(|item| item.contains("Payload")));
        assert!(
            violations
                .iter()
                .any(|item| item.contains("Event::payload"))
        );
        assert!(
            violations
                .iter()
                .any(|item| item.contains("Projection::Full::0"))
        );
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
    }
}
