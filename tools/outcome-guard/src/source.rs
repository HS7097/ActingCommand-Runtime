// SPDX-License-Identifier: AGPL-3.0-only

//! Source scanning with `syn`: the registry and key-table invocations (G1, G7), vocabulary
//! enums (G7) and the string channels a code or location must not travel through (G2-G5).
//! Test modules (`#[cfg(test)]`, `#[test]`) are skipped.

use proc_macro2::{Span, TokenStream, TokenTree};
use std::collections::{BTreeMap, BTreeSet};
use syn::parse::{ParseStream, Parser};
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, FnArg, GenericArgument, Ident, ImplItem, Item, ItemEnum, ItemImpl, Lit,
    LitStr, Pat, PathArguments, ReturnType, Signature, Stmt, Token, Type, TypeParamBound,
    Visibility, braced, parenthesized,
};

/// One `outcome_codes!` or `outcome_locations!` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    pub spelling: String,
    /// `None` for a location.
    pub category: Option<String>,
    pub line: usize,
}

/// One registry invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub name: String,
    pub locations: bool,
    pub entries: Vec<RegistryEntry>,
}

/// One `outcome_vocabulary!` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VocabularyEnum {
    pub name: String,
    pub vocabulary: String,
    pub tokens: Vec<String>,
}

/// One line of the `outcome_keys!` table, in catalog type spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyLine {
    pub name: String,
    pub kind: String,
    /// Record fields with `true` when required.
    pub fields: Vec<(String, bool)>,
}

/// What G1 and G7 need from one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MacroFacts {
    pub registries: Vec<Registry>,
    pub vocabularies: Vec<VocabularyEnum>,
    pub key_tables: Vec<Vec<KeyLine>>,
    pub errors: Vec<String>,
}

/// The four string-channel checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    /// A code or location carried as a string: field, parameter, `Result` error, getter.
    G2,
    /// A `json!` key that carries a code, category or location.
    G3,
    /// A `Code`, `Location` or `CodeStr` built outside the outcome module.
    G4,
    /// A string literal equal to a registered code or location outside its registry.
    G5,
}

/// One string-channel finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: Check,
    pub line: usize,
    /// The enclosing function, if any.
    pub item: Option<String>,
    pub detail: String,
}

const REGISTRY_MACROS: &[&str] = &[
    "outcome_codes",
    "outcome_locations",
    "outcome_vocabulary",
    "outcome_keys",
];

/// One out-of-line `mod name;` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDeclaration {
    /// The enclosing inline modules, then the declared name.
    pub names: Vec<String>,
    /// A `#[path = "..."]` attribute.
    pub path: Option<String>,
    /// Declared under `#[cfg(test)]`, here or in an enclosing inline module.
    pub test: bool,
}

/// Everything the guard reads from one file, from one parse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileScan {
    pub modules: Vec<ModuleDeclaration>,
    pub facts: MacroFacts,
    pub findings: Vec<Finding>,
}

/// What the string-channel checks need: the registered names, and whether the file belongs
/// to the outcome module (exempt from G4).
#[derive(Debug, Clone, Copy)]
pub struct ChannelScope<'a> {
    pub registered: &'a BTreeSet<String>,
    pub outcome_module: bool,
}

/// Parses one file once: its module declarations, its registry, vocabulary and key-table
/// invocations, and, when a scope is given, its string-channel findings.
pub fn scan_file(source: &str, channels: Option<ChannelScope<'_>>) -> Result<FileScan, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    let mut modules = Vec::new();
    collect_modules(&file.items, &mut Vec::new(), false, &mut modules);
    let mut macros = MacroVisitor::default();
    macros.visit_file(&file);
    let findings = match channels {
        None => Vec::new(),
        Some(scope) => {
            let mut visitor = ChannelVisitor {
                registered: scope.registered,
                outcome_module: scope.outcome_module,
                functions: Vec::new(),
                findings: Vec::new(),
            };
            visitor.visit_file(&file);
            visitor.findings
        }
    };
    Ok(FileScan {
        modules,
        facts: macros.facts,
        findings,
    })
}

fn collect_modules(
    items: &[Item],
    inline: &mut Vec<String>,
    in_test: bool,
    declarations: &mut Vec<ModuleDeclaration>,
) {
    for item in items {
        let Item::Mod(module) = item else {
            continue;
        };
        let test = in_test || has_cfg_test(&module.attrs);
        inline.push(module.ident.to_string());
        match &module.content {
            Some((_, items)) => collect_modules(items, inline, test, declarations),
            None => declarations.push(ModuleDeclaration {
                names: inline.clone(),
                path: path_attribute(&module.attrs),
                test,
            }),
        }
        inline.pop();
    }
}

fn path_attribute(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        match &attr.meta {
            syn::Meta::NameValue(value) => match &value.value {
                Expr::Lit(literal) => match &literal.lit {
                    Lit::Str(text) => Some(text.value()),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    })
}

#[derive(Default)]
struct MacroVisitor {
    facts: MacroFacts,
}

impl<'ast> Visit<'ast> for MacroVisitor {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !has_cfg_test(&node.attrs) {
            visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if !is_test_item(&node.attrs) {
            visit::visit_item_fn(self, node);
        }
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        if !has_cfg_test(&node.attrs) {
            visit::visit_item_impl(self, node);
        }
    }

    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        if node.ident.is_some() || has_cfg_test(&node.attrs) {
            return;
        }
        let Some(name) = node
            .mac
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
        else {
            return;
        };
        let line = line_of(node.mac.path.segments[0].ident.span());
        let tokens = node.mac.tokens.clone();
        let result = match name.as_str() {
            "outcome_codes" => parse_registry(tokens, true).map(|(registry_name, entries)| {
                self.facts.registries.push(Registry {
                    name: registry_name,
                    locations: false,
                    entries,
                })
            }),
            "outcome_locations" => parse_registry(tokens, false).map(|(registry_name, entries)| {
                self.facts.registries.push(Registry {
                    name: registry_name,
                    locations: true,
                    entries,
                })
            }),
            "outcome_vocabulary" => parse_vocabulary(tokens).map(|vocabulary| {
                self.facts.vocabularies.push(vocabulary);
            }),
            "outcome_keys" => parse_key_table(tokens).map(|keys| {
                self.facts.key_tables.push(keys);
            }),
            _ => Ok(()),
        };
        if let Err(error) = result {
            self.facts
                .errors
                .push(format!("line {line}: {name}! does not parse: {error}"));
        }
    }
}

type Entries = Vec<RegistryEntry>;

fn parse_registry(tokens: TokenStream, with_category: bool) -> syn::Result<(String, Entries)> {
    let parser = move |input: ParseStream| -> syn::Result<(String, Entries)> {
        input.call(Attribute::parse_outer)?;
        input.parse::<Visibility>()?;
        input.parse::<Token![enum]>()?;
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let entries = parse_entries(&content, with_category)?;
        Ok((name.to_string(), entries))
    };
    parser.parse2(tokens)
}

fn parse_vocabulary(tokens: TokenStream) -> syn::Result<VocabularyEnum> {
    let parser = |input: ParseStream| -> syn::Result<VocabularyEnum> {
        input.call(Attribute::parse_outer)?;
        input.parse::<Visibility>()?;
        input.parse::<Token![enum]>()?;
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let vocabulary: LitStr = input.parse()?;
        let content;
        braced!(content in input);
        let entries = parse_entries(&content, false)?;
        Ok(VocabularyEnum {
            name: name.to_string(),
            vocabulary: vocabulary.value(),
            tokens: entries.into_iter().map(|entry| entry.spelling).collect(),
        })
    };
    parser.parse2(tokens)
}

/// `closed_code!(Name { Variant => "token", ... })`.
fn parse_closed_code(tokens: TokenStream) -> syn::Result<(String, Vec<String>)> {
    let parser = |input: ParseStream| -> syn::Result<(String, Vec<String>)> {
        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);
        let entries = parse_entries(&content, false)?;
        Ok((
            name.to_string(),
            entries.into_iter().map(|entry| entry.spelling).collect(),
        ))
    };
    parser.parse2(tokens)
}

fn parse_entries(content: ParseStream, with_category: bool) -> syn::Result<Entries> {
    let mut entries = Vec::new();
    while !content.is_empty() {
        content.call(Attribute::parse_outer)?;
        content.parse::<Ident>()?;
        content.parse::<Token![=>]>()?;
        let spelling: LitStr = content.parse()?;
        let category = if with_category {
            content.parse::<Token![:]>()?;
            Some(content.parse::<Ident>()?.to_string())
        } else {
            None
        };
        entries.push(RegistryEntry {
            spelling: spelling.value(),
            category,
            line: line_of(spelling.span()),
        });
        if content.is_empty() {
            break;
        }
        content.parse::<Token![,]>()?;
    }
    Ok(entries)
}

fn parse_key_table(tokens: TokenStream) -> syn::Result<Vec<KeyLine>> {
    let parser = |input: ParseStream| -> syn::Result<Vec<KeyLine>> {
        let mut keys = Vec::new();
        while !input.is_empty() {
            let name: Ident = input.parse()?;
            input.parse::<Token![:]>()?;
            let (kind, fields) = parse_key_kind(input)?;
            keys.push(KeyLine {
                name: name.to_string(),
                kind,
                fields,
            });
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(keys)
    };
    parser.parse2(tokens)
}

fn parse_key_kind(input: ParseStream) -> syn::Result<(String, Vec<(String, bool)>)> {
    let kind: Ident = input.parse()?;
    match kind.to_string().as_str() {
        "vocab" => {
            let content;
            parenthesized!(content in input);
            let vocabulary: Ident = content.parse()?;
            Ok((format!("vocab:{vocabulary}"), Vec::new()))
        }
        "list" => {
            let content;
            parenthesized!(content in input);
            let (item, _) = parse_key_kind(&content)?;
            Ok((format!("list<{item}>"), Vec::new()))
        }
        "records" => {
            let content;
            parenthesized!(content in input);
            let mut fields = Vec::new();
            let mut required = true;
            while !content.is_empty() {
                if content.peek(Token![;]) {
                    content.parse::<Token![;]>()?;
                    required = false;
                    continue;
                }
                let field: Ident = content.parse()?;
                fields.push((field.to_string(), required));
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
            }
            Ok(("records".to_owned(), fields))
        }
        other => Ok((other.to_owned(), Vec::new())),
    }
}

/// The tokens of one enum in one file: from its `closed_code!` or `outcome_vocabulary!`
/// invocation, else from its `as_str` match, else from its serde names.
pub fn enum_tokens(source: &str, enum_name: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|error| error.to_string())?;
    let mut items = Vec::new();
    flatten_items(&file.items, &mut items);
    for item in &items {
        let Item::Macro(invocation) = item else {
            continue;
        };
        let macro_name = invocation
            .mac
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string());
        let parsed = match macro_name.as_deref() {
            Some("closed_code") => parse_closed_code(invocation.mac.tokens.clone()).ok(),
            Some("outcome_vocabulary") => parse_vocabulary(invocation.mac.tokens.clone())
                .ok()
                .map(|vocabulary| (vocabulary.name, vocabulary.tokens)),
            _ => None,
        };
        if let Some((name, tokens)) = parsed
            && name == enum_name
        {
            return Ok(tokens);
        }
    }
    let Some(definition) = items.iter().find_map(|item| match item {
        Item::Enum(definition) if definition.ident == enum_name => Some(definition),
        _ => None,
    }) else {
        return Err(format!("no enum {enum_name}"));
    };
    let spelled = as_str_arms(&items, enum_name);
    if definition
        .variants
        .iter()
        .all(|variant| spelled.contains_key(&variant.ident.to_string()))
    {
        return Ok(definition
            .variants
            .iter()
            .filter_map(|variant| spelled.get(&variant.ident.to_string()).cloned())
            .collect());
    }
    Ok(serde_tokens(definition))
}

fn flatten_items<'a>(items: &'a [Item], flat: &mut Vec<&'a Item>) {
    for item in items {
        match item {
            Item::Mod(module) if has_cfg_test(&module.attrs) => {}
            Item::Mod(module) => {
                if let Some((_, inner)) = &module.content {
                    flatten_items(inner, flat);
                }
            }
            other => flat.push(other),
        }
    }
}

fn as_str_arms(items: &[&Item], enum_name: &str) -> BTreeMap<String, String> {
    let mut spelled = BTreeMap::new();
    for item in items {
        let Item::Impl(implementation) = item else {
            continue;
        };
        if implementation.trait_.is_some()
            || type_name(&implementation.self_ty).as_deref() != Some(enum_name)
        {
            continue;
        }
        for member in &implementation.items {
            let ImplItem::Fn(function) = member else {
                continue;
            };
            if function.sig.ident != "as_str" {
                continue;
            }
            let Some(Stmt::Expr(Expr::Match(matching), _)) = function.block.stmts.last() else {
                continue;
            };
            for arm in &matching.arms {
                let Pat::Path(pattern) = &arm.pat else {
                    continue;
                };
                let (Some(variant), Some(literal)) =
                    (pattern.path.segments.last(), literal_text(&arm.body))
                else {
                    continue;
                };
                spelled.insert(variant.ident.to_string(), literal);
            }
        }
    }
    spelled
}

fn literal_text(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            Lit::Str(text) => Some(text.value()),
            _ => None,
        },
        Expr::Block(block) => match block.block.stmts.as_slice() {
            [Stmt::Expr(inner, None)] => literal_text(inner),
            _ => None,
        },
        _ => None,
    }
}

fn serde_tokens(definition: &ItemEnum) -> Vec<String> {
    let rename_all = serde_setting(&definition.attrs, "rename_all");
    definition
        .variants
        .iter()
        .map(|variant| {
            serde_setting(&variant.attrs, "rename").unwrap_or_else(|| {
                let name = variant.ident.to_string();
                match rename_all.as_deref() {
                    Some("snake_case") => snake_case(&name),
                    Some("lowercase") => name.to_lowercase(),
                    _ => name,
                }
            })
        })
        .collect()
}

/// `name = "value"` inside a `#[serde(...)]` attribute.
fn serde_setting(attrs: &[Attribute], setting: &str) -> Option<String> {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }
        let syn::Meta::List(list) = &attr.meta else {
            continue;
        };
        let tokens = list.tokens.clone().into_iter().collect::<Vec<_>>();
        for window in tokens.windows(3) {
            if let [
                TokenTree::Ident(name),
                TokenTree::Punct(equals),
                TokenTree::Literal(value),
            ] = window
                && name == setting
                && equals.as_char() == '='
                && let Some(text) = string_literal(value)
            {
                return Some(text);
            }
        }
    }
    None
}

fn snake_case(name: &str) -> String {
    let mut snake = String::new();
    for (index, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                snake.push('_');
            }
            snake.push(character.to_ascii_lowercase());
        } else {
            snake.push(character);
        }
    }
    snake
}

fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

/// The G2-G5 findings of one file.
pub fn inspect_channels(source: &str, scope: ChannelScope<'_>) -> Result<Vec<Finding>, String> {
    scan_file(source, Some(scope)).map(|scan| scan.findings)
}

struct ChannelVisitor<'a> {
    registered: &'a BTreeSet<String>,
    outcome_module: bool,
    functions: Vec<String>,
    findings: Vec<Finding>,
}

impl ChannelVisitor<'_> {
    fn report(&mut self, check: Check, span: Span, detail: String) {
        self.findings.push(Finding {
            check,
            line: line_of(span),
            item: self.functions.last().cloned(),
            detail,
        });
    }

    fn signature(&mut self, signature: &Signature) {
        let name = signature.ident.to_string();
        for input in &signature.inputs {
            let FnArg::Typed(typed) = input else {
                continue;
            };
            let Pat::Ident(binding) = typed.pat.as_ref() else {
                continue;
            };
            let parameter = binding.ident.to_string();
            if carries_code_name(&parameter) && is_string_type(&typed.ty) {
                self.report(
                    Check::G2,
                    binding.ident.span(),
                    format!("parameter {parameter} of {name} is a string"),
                );
            }
        }
        if let ReturnType::Type(_, output) = &signature.output {
            if result_error_is_string(output) {
                self.report(
                    Check::G2,
                    signature.ident.span(),
                    format!("{name} returns a Result whose error is a string"),
                );
            }
            if is_getter_name(&name) && is_string_type(output) {
                self.report(
                    Check::G2,
                    signature.ident.span(),
                    format!("getter {name} returns a string"),
                );
            }
        }
    }

    fn literal(&mut self, text: &str, span: Span) {
        if self.registered.contains(text) {
            self.report(
                Check::G5,
                span,
                format!("string literal \"{text}\" equals a registered name"),
            );
        }
    }

    fn macro_tokens(&mut self, tokens: TokenStream, json: bool) {
        let tokens = tokens.into_iter().collect::<Vec<_>>();
        for (index, token) in tokens.iter().enumerate() {
            match token {
                TokenTree::Group(group) => {
                    self.macro_tokens(group.stream(), json);
                }
                TokenTree::Literal(literal) => {
                    let Some(text) = string_literal(literal) else {
                        continue;
                    };
                    self.literal(&text, literal.span());
                    let followed_by_colon = matches!(
                        tokens.get(index + 1),
                        Some(TokenTree::Punct(punct)) if punct.as_char() == ':'
                    );
                    if json && followed_by_colon && is_json_key_name(&text) {
                        self.report(
                            Check::G3,
                            literal.span(),
                            format!("json! key \"{text}\" carries a code, category or location"),
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for ChannelVisitor<'_> {
    fn visit_attribute(&mut self, _node: &'ast Attribute) {}

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !has_cfg_test(&node.attrs) {
            visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if is_test_item(&node.attrs) {
            return;
        }
        self.functions.push(node.sig.ident.to_string());
        self.signature(&node.sig);
        visit::visit_item_fn(self, node);
        self.functions.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if is_test_item(&node.attrs) {
            return;
        }
        self.functions.push(node.sig.ident.to_string());
        self.signature(&node.sig);
        visit::visit_impl_item_fn(self, node);
        self.functions.pop();
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.functions.push(node.sig.ident.to_string());
        self.signature(&node.sig);
        visit::visit_trait_item_fn(self, node);
        self.functions.pop();
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        if has_cfg_test(&node.attrs) {
            return;
        }
        if !self.outcome_module {
            self.construction(node);
        }
        visit::visit_item_impl(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        if result_error_is_string(&node.ty) {
            self.report(
                Check::G2,
                node.ident.span(),
                format!("type {} is a Result whose error is a string", node.ident),
            );
        }
        visit::visit_item_type(self, node);
    }

    fn visit_field(&mut self, node: &'ast syn::Field) {
        if let Some(name) = &node.ident {
            let field = name.to_string();
            if carries_code_name(&field) && is_string_type(&node.ty) {
                self.report(Check::G2, name.span(), format!("field {field} is a string"));
            }
        }
        visit::visit_field(self, node);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        if !self.outcome_module
            && node
                .segments
                .iter()
                .any(|segment| segment.ident == "__from_registry")
        {
            let span = node.segments[0].ident.span();
            self.report(
                Check::G4,
                span,
                "a registry constructor is called outside a registry".to_owned(),
            );
        }
        visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let name = node
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        if REGISTRY_MACROS.contains(&name.as_str()) {
            return;
        }
        if !self.outcome_module && contains_ident(node.tokens.clone(), "__from_registry") {
            let span = node.path.segments[0].ident.span();
            self.report(
                Check::G4,
                span,
                "a registry constructor is called outside a registry".to_owned(),
            );
        }
        self.macro_tokens(node.tokens.clone(), name == "json");
    }

    fn visit_lit_str(&mut self, node: &'ast LitStr) {
        self.literal(&node.value(), node.span());
    }
}

impl ChannelVisitor<'_> {
    fn construction(&mut self, node: &ItemImpl) {
        let Some(target) = type_name(&node.self_ty) else {
            return;
        };
        if !matches!(target.as_str(), "Code" | "Location" | "CodeStr") {
            return;
        }
        if let Some((_, trait_path, _)) = &node.trait_ {
            let Some(last) = trait_path.segments.last() else {
                return;
            };
            let from_string = last.ident == "From"
                && match &last.arguments {
                    PathArguments::AngleBracketed(arguments) => {
                        arguments.args.iter().any(|argument| match argument {
                            GenericArgument::Type(ty) => is_string_type(ty) || is_str_reference(ty),
                            _ => false,
                        })
                    }
                    _ => false,
                };
            if from_string || last.ident == "FromStr" {
                self.report(
                    Check::G4,
                    last.ident.span(),
                    format!("{target} gets {} outside the outcome module", last.ident),
                );
            }
            return;
        }
        for member in &node.items {
            let ImplItem::Fn(function) = member else {
                continue;
            };
            let takes_string = function.sig.inputs.iter().any(|input| match input {
                FnArg::Typed(typed) => is_string_type(&typed.ty),
                FnArg::Receiver(_) => false,
            });
            if takes_string {
                self.report(
                    Check::G4,
                    function.sig.ident.span(),
                    format!(
                        "{target}::{} takes a string outside the outcome module",
                        function.sig.ident
                    ),
                );
            }
        }
    }
}

fn contains_ident(tokens: TokenStream, wanted: &str) -> bool {
    tokens.into_iter().any(|token| match token {
        TokenTree::Ident(ident) => ident == wanted,
        TokenTree::Group(group) => contains_ident(group.stream(), wanted),
        _ => false,
    })
}

fn string_literal(literal: &proc_macro2::Literal) -> Option<String> {
    match Lit::new(literal.clone()) {
        Lit::Str(text) => Some(text.value()),
        _ => None,
    }
}

/// `code`, `*_code`, `reason`, `*_reason`, `failure`, `operation`, `*_operation`, `stage`,
/// `*_stage` or `boundary`.
fn carries_code_name(name: &str) -> bool {
    matches!(
        name,
        "code" | "reason" | "failure" | "operation" | "stage" | "boundary"
    ) || ["_code", "_reason", "_operation", "_stage"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// `code`, `*_code`, `reason`, `*_reason`, `key`, `operation` or `stage`.
fn is_getter_name(name: &str) -> bool {
    matches!(name, "code" | "reason" | "key" | "operation" | "stage")
        || name.ends_with("_code")
        || name.ends_with("_reason")
}

/// `code`, `*_code`, `reason`, `*_reason`, `category`, `operation`, `*_operation`, `stage`,
/// `*_stage` or `boundary`.
fn is_json_key_name(name: &str) -> bool {
    (carries_code_name(name) && name != "failure") || name == "category"
}

fn is_str_reference(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => is_path_named(&reference.elem, "str"),
        Type::Paren(inner) => is_str_reference(&inner.elem),
        Type::Group(inner) => is_str_reference(&inner.elem),
        _ => false,
    }
}

fn is_path_named(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Path(path) => {
            path.qself.is_none()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == name)
        }
        _ => false,
    }
}

fn type_arguments(ty: &Type) -> Vec<&Type> {
    let Type::Path(path) = ty else {
        return Vec::new();
    };
    let Some(last) = path.path.segments.last() else {
        return Vec::new();
    };
    match &last.arguments {
        PathArguments::AngleBracketed(arguments) => arguments
            .args
            .iter()
            .filter_map(|argument| match argument {
                GenericArgument::Type(ty) => Some(ty),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `String`, `&str`, `Box<str>`, `Cow<str>`, `impl Into<String>`, `impl AsRef<str>`, or an
/// `Option` or `Vec` of these.
fn is_string_type(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => is_path_named(&reference.elem, "str"),
        Type::Paren(inner) => is_string_type(&inner.elem),
        Type::Group(inner) => is_string_type(&inner.elem),
        Type::ImplTrait(implementation) => implementation.bounds.iter().any(|bound| {
            let TypeParamBound::Trait(bound) = bound else {
                return false;
            };
            let Some(last) = bound.path.segments.last() else {
                return false;
            };
            let arguments = match &last.arguments {
                PathArguments::AngleBracketed(arguments) => arguments
                    .args
                    .iter()
                    .filter_map(|argument| match argument {
                        GenericArgument::Type(ty) => Some(ty),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            (last.ident == "Into" && arguments.iter().any(|ty| is_path_named(ty, "String")))
                || (last.ident == "AsRef" && arguments.iter().any(|ty| is_path_named(ty, "str")))
        }),
        Type::Path(path) if path.qself.is_none() => {
            let Some(last) = path.path.segments.last() else {
                return false;
            };
            let arguments = type_arguments(ty);
            match last.ident.to_string().as_str() {
                "String" => arguments.is_empty(),
                "Box" | "Cow" => arguments.iter().any(|ty| is_path_named(ty, "str")),
                "Option" | "Vec" => arguments.first().is_some_and(|ty| is_string_type(ty)),
                _ => false,
            }
        }
        _ => false,
    }
}

/// `Result<_, E>` with a string `E`.
fn result_error_is_string(ty: &Type) -> bool {
    if !is_path_named(ty, "Result") {
        return false;
    }
    let arguments = type_arguments(ty);
    arguments.len() == 2 && is_string_type(arguments[1])
}

/// A `#[cfg(...)]` that names `test`.
pub fn has_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && matches!(&attr.meta, syn::Meta::List(list) if contains_ident(list.tokens.clone(), "test"))
    })
}

fn is_test_item(attrs: &[Attribute]) -> bool {
    has_cfg_test(attrs) || attrs.iter().any(|attr| attr.path().is_ident("test"))
}

fn line_of(span: Span) -> usize {
    span.start().line
}
