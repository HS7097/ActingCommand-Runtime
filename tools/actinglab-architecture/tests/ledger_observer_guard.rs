// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #383 AO-3, test rows AO3-j and AO3-k (model-ledger-observer v1.4.1 §4.8, §6.1;
//! `contracts/ledger-observer.md` §2, §6, Appendix A): the ledger observer's architecture guard.
//!
//! Over the production code of runtime-host and actingd (test files, `tests` trees, modules
//! declared for tests only, items under a test-only `cfg` and functions named `*_for_test` left
//! out, as Appendix A.1 scopes it), every call of the ledger's write API, of the flush barrier,
//! of the ledger's reads, of the capture interface and of the tier read is listed by its
//! function. The guard keys on the receiver, not on a name pattern (Appendix A.8, M8): a call
//! counts when it is made on a `GlobalLedger`, on the maintenance writer (`LedgerMaintenance`) or
//! on the capture interface. A receiver is one of those when it is a parameter or a local of that
//! type (declared, or made by the type's constructor), or when it carries the type's binding
//! name (`ledger`, `maintenance`, `ledger_capture`); every field of those types carries that
//! name, which the guard checks. Each listed call must be on the whitelist with its count; the
//! whitelist started (AO-3) as the sites of Appendix A as the code stands and only shrinks. The
//! exception table of §6 is the write part of the whitelist for the offline subcommands: its rows
//! and the whitelist's W rows correspond one to one.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};

const GUARDED_ROOTS: [&str; 2] = ["crates/runtime-host/src", "apps/actingd/src"];

/// The ledger's write API (model §4.8 `append*`, `execute_critical`, `reconcile_*`, the
/// maintenance writer's `import`; Appendix A.8 M8 adds the writer-drafted ones).
const WRITE_METHODS: [&str; 11] = [
    "append",
    "append_with_observation",
    "append_deferred",
    "append_transaction",
    "append_durable",
    "append_with_source",
    "release_lab_pin",
    "admit_artifact_eviction",
    "finish_artifact_eviction",
    "reconcile_prior_epoch_closes",
    "reconcile_scheduled_policy_settlement",
];

/// The ledger's reads the guard keeps to their sites (model §4.8: `query`, `latest_sequence`).
const READ_METHODS: [&str; 3] = ["query", "query_page", "latest_sequence"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Write,
    Barrier,
    Read,
    Capture,
    Tier,
}

/// What a receiver holds: the ledger, the maintenance writer or the capture interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    Ledger,
    Maintenance,
    Capture,
}

impl Held {
    /// The binding name every field of the type carries.
    fn binding(self) -> &'static str {
        match self {
            Self::Ledger => "ledger",
            Self::Maintenance => "maintenance",
            Self::Capture => "ledger_capture",
        }
    }

    fn of_binding(name: &str) -> Option<Self> {
        [Self::Ledger, Self::Maintenance, Self::Capture]
            .into_iter()
            .find(|held| held.binding() == name)
    }
}

/// The type a binding holds, through references, parentheses and `Arc`, `Rc`, `Box` or
/// `Option`; `dyn`/`impl LedgerCapture` is the capture interface.
fn held_type(ty: &syn::Type) -> Option<Held> {
    match ty {
        syn::Type::Reference(reference) => held_type(&reference.elem),
        syn::Type::Paren(paren) => held_type(&paren.elem),
        syn::Type::Group(group) => held_type(&group.elem),
        syn::Type::Path(path) if path.qself.is_none() => {
            let segment = path.path.segments.last()?;
            match segment.ident.to_string().as_str() {
                "GlobalLedger" => Some(Held::Ledger),
                "LedgerMaintenance" => Some(Held::Maintenance),
                "LedgerCaptureHandle" => Some(Held::Capture),
                "Arc" | "Rc" | "Box" | "Option" => match &segment.arguments {
                    syn::PathArguments::AngleBracketed(arguments) if arguments.args.len() == 1 => {
                        match arguments.args.first() {
                            Some(syn::GenericArgument::Type(inner)) => held_type(inner),
                            _ => None,
                        }
                    }
                    _ => None,
                },
                _ => None,
            }
        }
        syn::Type::TraitObject(object) => capture_bound(object.bounds.iter()),
        syn::Type::ImplTrait(bounds) => capture_bound(bounds.bounds.iter()),
        _ => None,
    }
}

fn capture_bound<'a>(mut bounds: impl Iterator<Item = &'a syn::TypeParamBound>) -> Option<Held> {
    bounds
        .any(|bound| {
            matches!(bound, syn::TypeParamBound::Trait(bound)
                if bound.path.segments.last().is_some_and(|segment| segment.ident == "LedgerCapture"))
        })
        .then_some(Held::Capture)
}

/// The type an initializer makes: a constructor of `GlobalLedger` or `LedgerMaintenance`, or the
/// maintenance writer opening the ledger, through `?` and error mapping.
fn constructed(expr: &syn::Expr) -> Option<Held> {
    match expr {
        syn::Expr::Try(value) => constructed(&value.expr),
        syn::Expr::Paren(value) => constructed(&value.expr),
        syn::Expr::MethodCall(call) => match call.method.to_string().as_str() {
            "open_writer" | "open_writer_timed" => Some(Held::Ledger),
            "map_err" | "expect" | "unwrap" | "unwrap_or_else" => constructed(&call.receiver),
            _ => None,
        },
        syn::Expr::Call(call) => {
            let syn::Expr::Path(path) = &*call.func else {
                return None;
            };
            let names = path
                .path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>();
            match names.as_slice() {
                [.., owner, function] if owner == "GlobalLedger" => matches!(
                    function.as_str(),
                    "open"
                        | "open_with_artifact_verifier"
                        | "open_sqlite_candidate"
                        | "open_sqlite_candidate_with_artifact_verifier"
                )
                .then_some(Held::Ledger),
                [.., owner, function] if owner == "LedgerMaintenance" && function == "acquire" => {
                    Some(Held::Maintenance)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("architecture tool must live at tools/<name>")
        .to_path_buf()
}

fn collect_rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| entry.expect("directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// An item, statement or expression compiled only for tests: a `cfg` naming `test` (alone, in
/// `any`, `all`, or with a test-hook feature) and no `not`.
fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let syn::Meta::List(list) = &attr.meta else {
            return false;
        };
        let mut names = Vec::new();
        token_idents(list.tokens.clone(), &mut names);
        names.iter().any(|name| name == "test") && !names.iter().any(|name| name == "not")
    })
}

fn token_idents(tokens: proc_macro2::TokenStream, names: &mut Vec<String>) {
    for tree in tokens {
        match tree {
            proc_macro2::TokenTree::Ident(ident) => names.push(ident.to_string()),
            proc_macro2::TokenTree::Group(group) => token_idents(group.stream(), names),
            proc_macro2::TokenTree::Punct(_) | proc_macro2::TokenTree::Literal(_) => {}
        }
    }
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| match &attr.meta {
        syn::Meta::NameValue(value) if value.path.is_ident("path") => match &value.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(path),
                ..
            }) => Some(path.value()),
            _ => None,
        },
        _ => None,
    })
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// The files (and, ending in `/`, the directories) of the modules a file declares for tests
/// only (`#[cfg(test)] mod name;`).
fn test_modules(path: &str, file: &syn::File, excluded: &mut Vec<String>) {
    let (directory, name) = path.rsplit_once('/').expect("a file in a directory");
    let module_directory = match name {
        "main.rs" | "lib.rs" | "mod.rs" => directory.to_owned(),
        _ => format!("{directory}/{}", name.trim_end_matches(".rs")),
    };
    for item in &file.items {
        let syn::Item::Mod(module) = item else {
            continue;
        };
        if module.content.is_some() || !test_only(&module.attrs) {
            continue;
        }
        match path_attr(&module.attrs) {
            Some(explicit) => excluded.push(normalize(&format!("{directory}/{explicit}"))),
            None => {
                excluded.push(format!("{module_directory}/{}.rs", module.ident));
                excluded.push(format!("{module_directory}/{}/", module.ident));
            }
        }
    }
}

/// The guarded production sources, parsed, as (workspace-relative path, file).
fn guarded_sources(root: &Path) -> Vec<(String, syn::File)> {
    let mut parsed = Vec::new();
    for directory in GUARDED_ROOTS {
        let mut files = Vec::new();
        collect_rust_files(&root.join(directory), &mut files);
        for file in files {
            let path = file
                .strip_prefix(root)
                .expect("a workspace file")
                .components()
                .map(|component| component.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let name = path.rsplit('/').next().expect("a file name");
            if path.split('/').any(|component| component == "tests")
                || name == "tests.rs"
                || name.ends_with("_tests.rs")
                || name.starts_with("test_")
            {
                continue;
            }
            let source =
                fs::read_to_string(&file).unwrap_or_else(|error| panic!("read {path}: {error}"));
            let syntax =
                syn::parse_file(&source).unwrap_or_else(|error| panic!("parse {path}: {error}"));
            parsed.push((path, syntax));
        }
    }
    let mut excluded = Vec::new();
    for (path, syntax) in &parsed {
        test_modules(path, syntax, &mut excluded);
    }
    let sources = parsed
        .into_iter()
        .filter(|(path, _)| {
            !excluded
                .iter()
                .any(|excluded| match excluded.strip_suffix('/') {
                    Some(_) => path.starts_with(excluded.as_str()),
                    None => path == excluded,
                })
        })
        .collect::<Vec<_>>();
    assert!(!sources.is_empty(), "no guarded production sources");
    sources
}

fn expr_attrs(expr: &syn::Expr) -> &[syn::Attribute] {
    use syn::Expr;
    match expr {
        Expr::Array(value) => &value.attrs,
        Expr::Assign(value) => &value.attrs,
        Expr::Async(value) => &value.attrs,
        Expr::Await(value) => &value.attrs,
        Expr::Binary(value) => &value.attrs,
        Expr::Block(value) => &value.attrs,
        Expr::Break(value) => &value.attrs,
        Expr::Call(value) => &value.attrs,
        Expr::Cast(value) => &value.attrs,
        Expr::Closure(value) => &value.attrs,
        Expr::Const(value) => &value.attrs,
        Expr::Continue(value) => &value.attrs,
        Expr::Field(value) => &value.attrs,
        Expr::ForLoop(value) => &value.attrs,
        Expr::Group(value) => &value.attrs,
        Expr::If(value) => &value.attrs,
        Expr::Index(value) => &value.attrs,
        Expr::Infer(value) => &value.attrs,
        Expr::Let(value) => &value.attrs,
        Expr::Lit(value) => &value.attrs,
        Expr::Loop(value) => &value.attrs,
        Expr::Macro(value) => &value.attrs,
        Expr::Match(value) => &value.attrs,
        Expr::MethodCall(value) => &value.attrs,
        Expr::Paren(value) => &value.attrs,
        Expr::Path(value) => &value.attrs,
        Expr::Range(value) => &value.attrs,
        Expr::Reference(value) => &value.attrs,
        Expr::Repeat(value) => &value.attrs,
        Expr::Return(value) => &value.attrs,
        Expr::Struct(value) => &value.attrs,
        Expr::Try(value) => &value.attrs,
        Expr::TryBlock(value) => &value.attrs,
        Expr::Tuple(value) => &value.attrs,
        Expr::Unary(value) => &value.attrs,
        Expr::Unsafe(value) => &value.attrs,
        Expr::While(value) => &value.attrs,
        Expr::Yield(value) => &value.attrs,
        _ => &[],
    }
}

/// The last name of a receiver: a binding (`ledger`) or a field (`self.ledger`,
/// `shared.ledger`); `None` for any other expression.
fn receiver_name(receiver: &syn::Expr) -> Option<String> {
    match receiver {
        syn::Expr::Path(path) if path.qself.is_none() => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        syn::Expr::Field(field) => match &field.member {
            syn::Member::Named(name) => Some(name.to_string()),
            syn::Member::Unnamed(_) => None,
        },
        _ => None,
    }
}

fn type_name(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Reference(reference) => type_name(&reference.elem),
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

/// Collects the guarded calls of one file by function, and the fields of the guarded types.
struct Scan<'a> {
    path: &'a str,
    owner: Option<String>,
    function: Option<String>,
    /// The function's parameters and locals of a guarded type, by name.
    locals: Vec<(String, Held)>,
    rows: BTreeMap<(Kind, String), usize>,
    /// Fields of a guarded type that do not carry its binding name: (`path::Struct`, field).
    misnamed_fields: Vec<String>,
}

impl<'a> Scan<'a> {
    fn new(path: &'a str) -> Self {
        Self {
            path,
            owner: None,
            function: None,
            locals: Vec::new(),
            rows: BTreeMap::new(),
            misnamed_fields: Vec::new(),
        }
    }

    fn site(&self) -> Option<String> {
        let function = self.function.as_ref()?;
        Some(match &self.owner {
            Some(owner) => format!("{}::{owner}::{function}", self.path),
            None => format!("{}::{function}", self.path),
        })
    }

    fn record(&mut self, kind: Kind, api: &str) {
        if let Some(site) = self.site() {
            *self
                .rows
                .entry((kind, format!("{site} -> {api}")))
                .or_default() += 1;
        }
    }

    fn held(&self, receiver: &syn::Expr) -> Option<Held> {
        let name = receiver_name(receiver)?;
        if let Some(held) = Held::of_binding(&name) {
            return Some(held);
        }
        match receiver {
            syn::Expr::Path(path) if path.path.segments.len() == 1 => self
                .locals
                .iter()
                .rev()
                .find(|(local, _)| *local == name)
                .map(|(_, held)| *held),
            _ => None,
        }
    }

    fn classify(&mut self, method: &str, held: Option<Held>) {
        let kind = match held {
            Some(Held::Ledger) if WRITE_METHODS.contains(&method) => Some(Kind::Write),
            Some(Held::Maintenance) if method == "import" => Some(Kind::Write),
            Some(Held::Ledger) if READ_METHODS.contains(&method) => Some(Kind::Read),
            _ if method == "wait_committed" => Some(Kind::Barrier),
            Some(Held::Capture) => Some(Kind::Capture),
            Some(Held::Ledger) if method == "capture_handle" => Some(Kind::Capture),
            Some(Held::Ledger) if method == "capture_status" => Some(Kind::Tier),
            _ => None,
        };
        if let Some(kind) = kind {
            self.record(kind, method);
        }
    }

    fn local(&mut self, pattern: &syn::Pat, held: Option<Held>) {
        if let (syn::Pat::Ident(pattern), Some(held)) = (pattern, held) {
            self.locals.push((pattern.ident.to_string(), held));
        }
    }

    fn function<F: FnOnce(&mut Self)>(&mut self, name: String, attrs: &[syn::Attribute], body: F) {
        if test_only(attrs) || name.ends_with("_for_test") {
            return;
        }
        let saved = self.function.replace(name);
        let saved_locals = std::mem::take(&mut self.locals);
        body(self);
        self.locals = saved_locals;
        self.function = saved;
    }
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        let saved_owner = self.owner.take();
        self.function(item.sig.ident.to_string(), &item.attrs, |scan| {
            visit::visit_item_fn(scan, item);
        });
        self.owner = saved_owner;
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        self.function(item.sig.ident.to_string(), &item.attrs, |scan| {
            visit::visit_impl_item_fn(scan, item);
        });
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        self.function(item.sig.ident.to_string(), &item.attrs, |scan| {
            visit::visit_trait_item_fn(scan, item);
        });
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        if test_only(&item.attrs) {
            return;
        }
        let saved_owner = self
            .owner
            .replace(type_name(&item.self_ty).unwrap_or_default());
        let saved_function = self.function.take();
        visit::visit_item_impl(self, item);
        self.function = saved_function;
        self.owner = saved_owner;
    }

    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        if test_only(&item.attrs) {
            return;
        }
        let saved_owner = self.owner.replace(item.ident.to_string());
        let saved_function = self.function.take();
        visit::visit_item_trait(self, item);
        self.function = saved_function;
        self.owner = saved_owner;
    }

    fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
        if test_only(&item.attrs) {
            return;
        }
        let saved_owner = self.owner.take();
        let saved_function = self.function.take();
        visit::visit_item_mod(self, item);
        self.function = saved_function;
        self.owner = saved_owner;
    }

    fn visit_item_struct(&mut self, item: &'ast syn::ItemStruct) {
        if test_only(&item.attrs) {
            return;
        }
        for (index, field) in item.fields.iter().enumerate() {
            if test_only(&field.attrs) {
                continue;
            }
            let Some(held) = held_type(&field.ty) else {
                continue;
            };
            let name = field
                .ident
                .as_ref()
                .map_or_else(|| index.to_string(), ToString::to_string);
            if name != held.binding() {
                self.misnamed_fields.push(format!(
                    "{}::{}.{name} (a {held:?} field must be named `{}`)",
                    self.path,
                    item.ident,
                    held.binding()
                ));
            }
        }
    }

    fn visit_item_const(&mut self, item: &'ast syn::ItemConst) {
        if test_only(&item.attrs) || self.function.is_none() {
            return;
        }
        visit::visit_item_const(self, item);
    }

    fn visit_item_static(&mut self, item: &'ast syn::ItemStatic) {
        if test_only(&item.attrs) || self.function.is_none() {
            return;
        }
        visit::visit_item_static(self, item);
    }

    fn visit_item_use(&mut self, _item: &'ast syn::ItemUse) {}

    fn visit_fn_arg(&mut self, arg: &'ast syn::FnArg) {
        if let syn::FnArg::Typed(typed) = arg {
            self.local(&typed.pat, held_type(&typed.ty));
        }
        visit::visit_fn_arg(self, arg);
    }

    fn visit_local(&mut self, local: &'ast syn::Local) {
        if test_only(&local.attrs) {
            return;
        }
        visit::visit_local(self, local);
        match &local.pat {
            syn::Pat::Type(typed) => self.local(&typed.pat, held_type(&typed.ty)),
            pattern => {
                let held = local.init.as_ref().and_then(|init| constructed(&init.expr));
                self.local(pattern, held);
            }
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        match stmt {
            syn::Stmt::Macro(value) if test_only(&value.attrs) => {}
            _ => visit::visit_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'ast syn::Expr) {
        if test_only(expr_attrs(expr)) {
            return;
        }
        visit::visit_expr(self, expr);
    }

    fn visit_field_value(&mut self, value: &'ast syn::FieldValue) {
        if test_only(&value.attrs) {
            return;
        }
        visit::visit_field_value(self, value);
    }

    fn visit_arm(&mut self, arm: &'ast syn::Arm) {
        if test_only(&arm.attrs) {
            return;
        }
        visit::visit_arm(self, arm);
    }

    fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
        let held = self.held(&call.receiver);
        self.classify(&call.method.to_string(), held);
        visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_field(&mut self, field: &'ast syn::ExprField) {
        if let syn::Member::Named(name) = &field.member
            && name == Held::Capture.binding()
        {
            self.record(Kind::Capture, ".ledger_capture");
        }
        visit::visit_expr_field(self, field);
    }

    fn visit_expr_call(&mut self, call: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = &*call.func
            && path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "execute_critical")
        {
            self.record(Kind::Write, "execute_critical");
        }
        visit::visit_expr_call(self, call);
    }
}

struct Scanned {
    rows: BTreeMap<(Kind, String), usize>,
    misnamed_fields: Vec<String>,
}

fn scan(sources: &[(String, syn::File)]) -> Scanned {
    let mut rows = BTreeMap::new();
    let mut misnamed_fields = Vec::new();
    for (path, file) in sources {
        let mut scan = Scan::new(path);
        scan.visit_file(file);
        for (key, count) in scan.rows {
            *rows.entry(key).or_default() += count;
        }
        misnamed_fields.extend(scan.misnamed_fields);
    }
    Scanned {
        rows,
        misnamed_fields,
    }
}

fn of_kind(rows: &BTreeMap<(Kind, String), usize>, kind: Kind) -> BTreeMap<String, usize> {
    rows.iter()
        .filter(|((row_kind, _), _)| *row_kind == kind)
        .map(|((_, row), count)| (row.clone(), *count))
        .collect()
}

fn table_rows(table: &[(&str, usize, &str)]) -> BTreeMap<String, usize> {
    let mut rows = BTreeMap::new();
    for (row, count, label) in table {
        assert!(!label.is_empty(), "the whitelist row {row} has no reason");
        assert!(
            rows.insert((*row).to_owned(), *count).is_none(),
            "the whitelist names {row} twice"
        );
    }
    rows
}

fn differences(
    kind: Kind,
    actual: &BTreeMap<String, usize>,
    allowed: &[(&str, usize, &str)],
) -> Vec<String> {
    let allowed = table_rows(allowed);
    let mut problems = Vec::new();
    for (row, count) in actual {
        match allowed.get(row) {
            None => problems.push(format!("{kind:?} not on the whitelist: {row} (x{count})")),
            Some(limit) if limit != count => problems.push(format!(
                "{kind:?} {row}: the code makes {count}, the whitelist allows {limit}"
            )),
            Some(_) => {}
        }
    }
    for (row, count) in &allowed {
        if !actual.contains_key(row) {
            problems.push(format!(
                "{kind:?} whitelist row without a site (shrink the whitelist): {row} (x{count})"
            ));
        }
    }
    problems
}

fn whitelist(kind: Kind) -> &'static [(&'static str, usize, &'static str)] {
    match kind {
        Kind::Write => WRITE_SITES,
        Kind::Barrier => BARRIER_SITES,
        Kind::Read => READ_SITES,
        Kind::Capture => CAPTURE_SITES,
        Kind::Tier => TIER_SITES,
    }
}

const KINDS: [Kind; 5] = [
    Kind::Write,
    Kind::Barrier,
    Kind::Read,
    Kind::Capture,
    Kind::Tier,
];

/// AO3-j: in the non-test code of runtime-host and actingd, the calls of the ledger's write API,
/// of the flush barrier, of the ledger's reads, of the capture interface and of the tier read
/// are exactly the whitelist's sites; a call added outside the whitelist fails the guard. The
/// guard keys on the receiver's type, which a small source checks before the workspace.
#[test]
fn ao3_j_ledger_calls_stay_on_the_whitelist_of_the_observer_guard() {
    // The guard itself: writes through a parameter and a local of another name, a write and a
    // read through the binding names, a capture, a barrier and a misnamed field are found;
    // test-only code and a `Vec::append` are not.
    let probe = syn::parse_file(
        r#"
        struct Holder<'a> {
            ledger: &'a GlobalLedger,
            store: Arc<GlobalLedger>,
            ledger_capture: LedgerCaptureHandle,
        }
        impl Holder<'_> {
            fn write(&self, records: &mut Vec<u8>, draft: Draft) {
                self.ledger.append(draft);
                records.append(&mut Vec::new());
                self.ledger_capture.capture(item());
            }
        }
        fn through(store: &GlobalLedger, draft: Draft) {
            store.append(draft);
            let maintenance_writer = LedgerMaintenance::acquire(root)?;
            maintenance_writer.import(draft);
            barrier.wait_committed(ticket);
        }
        fn helper_for_test(ledger: &GlobalLedger) { ledger.append(draft()); }
        #[cfg(test)]
        fn tested(ledger: &GlobalLedger) { ledger.query(query()); }
        fn reads(shared: &Shared) { shared.ledger.latest_sequence(); }
        "#,
    )
    .expect("the probe parses");
    let probed = scan(&[("probe.rs".to_owned(), probe)]);
    let found = probed
        .rows
        .iter()
        .map(|((kind, row), count)| format!("{kind:?} {row} x{count}"))
        .collect::<BTreeSet<_>>();
    let expected = [
        "Write probe.rs::Holder::write -> append x1",
        "Capture probe.rs::Holder::write -> .ledger_capture x1",
        "Capture probe.rs::Holder::write -> capture x1",
        "Write probe.rs::through -> append x1",
        "Write probe.rs::through -> import x1",
        "Barrier probe.rs::through -> wait_committed x1",
        "Read probe.rs::reads -> latest_sequence x1",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<BTreeSet<_>>();
    assert_eq!(found, expected, "the guard's own probe");
    assert_eq!(
        probed.misnamed_fields,
        vec!["probe.rs::Holder.store (a Ledger field must be named `ledger`)".to_owned()],
        "the guard's field check"
    );

    let scanned = scan(&guarded_sources(&workspace_root()));
    let mut problems = scanned.misnamed_fields.clone();
    for kind in KINDS {
        problems.extend(differences(
            kind,
            &of_kind(&scanned.rows, kind),
            whitelist(kind),
        ));
    }
    assert!(
        problems.is_empty(),
        "ledger observer guard (contracts/ledger-observer.md §2, Appendix A; AO3-j):\n{}",
        problems.join("\n")
    );
}

/// One row of the exception table (§6): its number, the runtime-host file it writes from and the
/// write call.
struct ExceptionRow {
    number: String,
    file: String,
    api: &'static str,
}

fn exception_table(contract: &str) -> Vec<ExceptionRow> {
    let start = contract
        .find("\n## 6 ")
        .expect("contracts/ledger-observer.md has §6");
    let section = &contract[start + 1..];
    let section = &section[..section.find("\n## ").expect("§6 ends at the next section")];
    let mut rows = Vec::new();
    for line in section.lines() {
        let Some(rest) = line.strip_prefix("| W") else {
            continue;
        };
        let number = format!(
            "W{}",
            rest.split(' ').next().expect("an exception row number")
        );
        let files = line
            .match_indices("crates/runtime-host/src/")
            .map(|(at, _)| {
                let tail = &line[at..];
                tail[..tail.find(".rs").expect("a Rust file") + 3].to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            files.len(),
            1,
            "{number} names one runtime-host file: {files:?}"
        );
        let apis = [
            ("`ledger.append`", "append"),
            ("LedgerMaintenance::import", "import"),
        ]
        .into_iter()
        .filter(|(text, _)| line.contains(text))
        .map(|(_, api)| api)
        .collect::<Vec<_>>();
        assert_eq!(apis.len(), 1, "{number} names one write call: {apis:?}");
        rows.push(ExceptionRow {
            number,
            file: files[0].clone(),
            api: apis[0],
        });
    }
    rows
}

/// AO3-k: the exception table of §6 and the whitelist's W rows correspond row for row: every
/// row of the table is a whitelisted write at its file with its call, every W row of the
/// whitelist is a row of the table, and the offline subcommands' files write nowhere else. Until
/// AO-4 the rest of the write whitelist is the transitional sites of Appendix A, each with the
/// reason it stays; AO3-j fails any write added outside it.
#[test]
fn ao3_k_the_exception_table_and_the_whitelist_s_w_rows_correspond_row_for_row() {
    let contract_path = workspace_root().join("contracts/ledger-observer.md");
    let contract = fs::read_to_string(&contract_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", contract_path.display()));
    let table = exception_table(&contract);
    assert!(!table.is_empty(), "§6 lists the exception rows");
    let numbers = table
        .iter()
        .map(|row| row.number.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(numbers.len(), table.len(), "§6 numbers each row once");

    let mut problems = Vec::new();
    for row in &table {
        let matching = WRITE_SITES
            .iter()
            .filter(|(site, _, label)| {
                *label == row.number
                    && site.starts_with(&format!("{}::", row.file))
                    && site.ends_with(&format!(" -> {}", row.api))
            })
            .count();
        if matching != 1 {
            problems.push(format!(
                "{} ({} `{}`) has {matching} whitelist rows, not 1",
                row.number, row.file, row.api
            ));
        }
    }
    for (site, _, label) in WRITE_SITES {
        let in_offline_file = table
            .iter()
            .any(|row| site.starts_with(&format!("{}::", row.file)));
        let numbered = label.starts_with('W');
        if numbered && !numbers.contains(*label) {
            problems.push(format!(
                "{site}: {label} is not a row of the exception table"
            ));
        }
        if in_offline_file && !numbered {
            problems.push(format!(
                "{site}: an offline subcommand writes outside its exception row"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "exception table against the whitelist (contracts/ledger-observer.md §6; AO3-k):\n{}",
        problems.join("\n")
    );
}

// The whitelist (AO-3: the sites of Appendix A as the code stands; only shrinks). Each row:
// (`file::Owner::function -> call`, the calls there, why it stays and the slice that moves it).
const WRITE_SITES: &[(&str, usize, &str)] = &[
    (
        "crates/runtime-host/src/host.rs::RuntimeHost::start_with_provider -> reconcile_prior_epoch_closes",
        1,
        "ledger-own trigger: prior-epoch imports and scope closes (A.8 M3)",
    ),
    (
        "crates/runtime-host/src/host/agent_control.rs::append_agent_wake -> append",
        1,
        "G12; its record passes the capture's observation step; AO-4",
    ),
    (
        "crates/runtime-host/src/host/client_events.rs::HostShared::release_lab_pin -> release_lab_pin",
        1,
        "G9 (A.8 M4), the ledger drafts the record; AO-4",
    ),
    (
        "crates/runtime-host/src/host/contained_task.rs::RuntimeArtifactEventSink::append -> append",
        1,
        "G9 artifact sink; AO-4",
    ),
    (
        "crates/runtime-host/src/host/device_diagnostic.rs::append_device_diagnostic_record -> append_deferred",
        1,
        "G16, the summary of a start that ended before the host was shared",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::CapturingAppender::append_durable -> append_durable",
        1,
        "capture point: the critical executions' appender (G4, G6) until AO-4",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::append_event_under_fact_gate_with_observation -> append",
        1,
        "capture point: the fan-out point (G1)",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::append_event_under_fact_gate_with_observation -> append_with_observation",
        1,
        "capture point: the fan-out point (G1), observed",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::capture_deferred_under_gate -> append_deferred",
        1,
        "capture point: the close summary (G16)",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::capture_store_change_under_gate -> append_with_source",
        1,
        "capture point: a store change whose receipt names the ledger sequence until AO-4 (G2)",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::synchronize_fact_store_under_gate -> append",
        1,
        "capture point: the invalidations the start's replay derived (until AO-6)",
    ),
    (
        "crates/runtime-host/src/host/frame_retention.rs::EvictionRecovery::round -> admit_artifact_eviction",
        1,
        "ledger-own trigger: startup eviction recovery (A.8 M3)",
    ),
    (
        "crates/runtime-host/src/host/frame_retention.rs::EvictionRecovery::round -> finish_artifact_eviction",
        1,
        "ledger-own trigger: startup eviction recovery (A.8 M3)",
    ),
    (
        "crates/runtime-host/src/host/frame_retention.rs::pin_published_frames -> append",
        1,
        "G9 frame pin; AO-4",
    ),
    (
        "crates/runtime-host/src/host/governance.rs::HostShared::append_approval_decision -> append_transaction",
        1,
        "G15, handed off through the capture point; AO-4",
    ),
    (
        "crates/runtime-host/src/host/input.rs::HostShared::application_control -> execute_critical",
        1,
        "G6 through the capturing appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/input.rs::HostShared::input -> execute_critical",
        1,
        "G6 through the capturing appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/lease.rs::HostShared::cleanup_token_guarded -> execute_critical",
        1,
        "G4 through the capturing appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/lease.rs::HostShared::grant_prepared_lease_with_links -> execute_critical",
        1,
        "G4 through the capturing appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/lease.rs::HostShared::release_lease -> execute_critical",
        1,
        "G4 through the capturing appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/lease.rs::HostShared::renew_held_lease -> execute_critical",
        1,
        "G4 through the capturing appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/lease.rs::HostShared::renew_lease -> execute_critical",
        1,
        "G4 through the capturing appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/lifecycle.rs::HostShared::append_instance_binding_events -> append",
        1,
        "G16, handed off through the capture point",
    ),
    (
        "crates/runtime-host/src/host/lifecycle.rs::HostShared::append_runtime_start_event -> append",
        1,
        "G16, handed off through the capture point",
    ),
    (
        "crates/runtime-host/src/host/online_observation.rs::ObservationArtifactSink::append -> append",
        1,
        "G9 artifact sink; AO-4",
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::record_policy_planning_signal -> append_transaction",
        1,
        "G15, handed off through the capture point; AO-4",
    ),
    (
        "crates/runtime-host/src/host/policy_catalog.rs::HostShared::switch_policy_catalog -> append",
        2,
        "G15 catalog transition; observed after the commit; AO-4",
    ),
    (
        "crates/runtime-host/src/host/policy_catalog.rs::HostShared::switch_policy_catalog -> append_transaction",
        1,
        "G15 catalog transition; observed after the commit; AO-4",
    ),
    (
        "crates/runtime-host/src/host/policy_dispatch.rs::HostShared::admit_policy_dispatch -> execute_critical",
        1,
        "G5 admission; observed after the commit; AO-4",
    ),
    (
        "crates/runtime-host/src/host/policy_dispatch.rs::PolicyAdmissionAppender::append_durable -> append_durable",
        1,
        "G5 admission appender; AO-4",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::reconcile_pending_policy_settlements -> reconcile_scheduled_policy_settlement",
        1,
        "settlement continuation; its records pass the observation step; AO-4 retires it",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::reconcile_policy_dispatches -> append",
        1,
        "G5 at start; AO-6 moves it to step 9",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::reconcile_policy_dispatches -> reconcile_scheduled_policy_settlement",
        1,
        "settlement continuation at start; AO-4 retires it",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::reconcile_scheduled_policy_outcomes_for -> reconcile_scheduled_policy_settlement",
        1,
        "settlement continuation; its records pass the observation step; AO-4 retires it",
    ),
    (
        "crates/runtime-host/src/host/runtime_facts.rs::append_takeover_invalidations -> append",
        1,
        "G3 at start (workbench recovery); AO-6",
    ),
    (
        "crates/runtime-host/src/host/runtime_facts.rs::recover_runtime_fact_store -> append",
        1,
        "G16 at start (workbench recovery); AO-6",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::HostShared::stage_release_set -> append_transaction",
        1,
        "G15 release staging, handed off through the capture point; AO-4",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::HostShared::switch_release_set -> append",
        2,
        "G15 release transition; observed after the commit; AO-4",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::HostShared::switch_release_set -> append_transaction",
        1,
        "G15 release transition; observed after the commit; AO-4",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::append_runtime_state_event -> append",
        1,
        "G15 at start (state migration); AO-4",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::reconcile_release_state -> append_transaction",
        2,
        "G15 at start (release baseline); AO-4",
    ),
    (
        "crates/runtime-host/src/ledger_maintenance.rs::run_locked -> import",
        1,
        "W2",
    ),
    (
        "crates/runtime-host/src/monitor.rs::MonitorRegistry::open -> append",
        1,
        "G16 at start (A.8 M9); AO-6",
    ),
    (
        "crates/runtime-host/src/monitor.rs::MonitorRegistry::recover -> append",
        1,
        "G16 at start (A.8 M9); AO-6",
    ),
    (
        "crates/runtime-host/src/owner_unlock.rs::record -> append",
        1,
        "W1",
    ),
    (
        "crates/runtime-host/src/performance/capacity.rs::append -> append",
        1,
        "G11 capacity records; AO-4",
    ),
    (
        "crates/runtime-host/src/policy_host/catalog_transaction.rs::CatalogStore::migrate_legacy_active_pointer -> append_transaction",
        1,
        "G15 at start (catalog pointer migration); AO-4",
    ),
    (
        "crates/runtime-host/src/provider_startup.rs::ProviderStartup::record -> append",
        1,
        "G10 at start (A.8 M2); AO-4",
    ),
];

const BARRIER_SITES: &[(&str, usize, &str)] = &[];

const READ_SITES: &[(&str, usize, &str)] = &[
    (
        "crates/runtime-host/src/agent_dispatcher.rs::read_recovery_events -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/approval.rs::ApprovalProjection::extend -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/approval.rs::ApprovalProjection::project -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/fact_store.rs::InstanceFactStore::active_records_at -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/fact_store.rs::InstanceFactStore::active_records_at -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/fact_store.rs::InstanceFactStore::at_position -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/fact_store.rs::InstanceFactStore::at_position -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/fact_store.rs::InstanceFactStore::recover -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/fact_store.rs::InstanceFactStore::synchronize -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/fact_store.rs::original_at -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/agent_control.rs::reconcile_agent_wakes -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/client_events.rs::HostShared::client_fact_replay -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/contained_task.rs::HostShared::cancel_contained_task -> query",
        2,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/contained_task.rs::HostShared::check_contained_task_terminal -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/contained_task.rs::HostShared::recover_contained_task -> query",
        2,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/contained_task.rs::HostShared::settle_previous_epoch_contained_run -> query",
        3,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/contained_task.rs::HostShared::settle_previous_epoch_contained_runs -> query",
        2,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::instance_fact_snapshot -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/failure_settlement.rs::HostShared::build_scheduled_failure_identity -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/failure_settlement.rs::HostShared::compare_error_frames -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/failure_settlement.rs::HostShared::compare_error_frames -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/input.rs::HostShared::recover_application_lifecycle -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/input.rs::HostShared::recover_safe_reset -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/installation.rs::HostShared::recover_install_pauses -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/installation.rs::HostShared::recover_install_pauses -> query_page",
        2,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/lab_operation.rs::HostShared::resolve_lab_input -> query_page",
        3,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/lease.rs::HostShared::existing_lease_terminal -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/lease.rs::HostShared::query_single_terminal -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/online_observation.rs::HostShared::check_observation_sample_inputs -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/planning.rs::HostShared::read_verified_artifacts -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_dispatch.rs::HostShared::admit_policy_dispatch -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_dispatch.rs::HostShared::project_authoritative_policy_inputs_with_gaps_under_gate -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_dispatch.rs::HostShared::project_policy_forward -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::complete_scheduled_policy_run -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::ensure_scheduled_policy_lease_released -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::finish_policy_dispatch_outcome -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::hold_unsettled_dispatches -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::inactive_policy_settlements -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::inactive_policy_settlements -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::read_scheduled_policy_outcome -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::read_scheduled_policy_outcome -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::record_open_scheduled_dispatches -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::recover_unreleased_policy_runs -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::recover_unreleased_policy_runs -> query",
        2,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::scheduled_policy_failure_input -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::validate_scheduled_policy_terminal -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::validate_scheduled_policy_terminal -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::HostShared::validate_scheduled_policy_terminal -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::linked_policy_run_events -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::reconcile_policy_dispatches -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::reconcile_scheduled_policy_outcomes_for -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::recover_authoritative_policy_outcomes -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::recover_remedy_state -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::recover_scheduled_terminal -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/policy_outcome.rs::validate_policy_run_admission_request -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/read_events.rs::HostShared::observe_runtime_state -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/read_events.rs::HostShared::project_interface -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/read_events.rs::HostShared::recent_project_diagnostics -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/runtime_config.rs::HostShared::record_config_catalog -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/runtime_facts.rs::HostShared::append_runtime_fact_snapshot_if_dirty -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/runtime_facts.rs::HostShared::runtime_fact_snapshot -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/runtime_facts.rs::SnapshotEventsNewestFirst::next -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/runtime_facts.rs::recover_runtime_fact_store -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/runtime_facts.rs::recover_runtime_fact_store -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/signatures.rs::HostShared::current_signature_catalog -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::reconcile_runtime_state -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::visit_release_facts -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/state_control.rs::visit_release_facts -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/task_diagnostic.rs::RuntimeContainedTask::finish_diagnostic -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/host/task_diagnostic.rs::RuntimeContainedTask::finish_diagnostic -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/monitor.rs::MonitorRegistry::recover -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/monitor.rs::MonitorRegistry::recover -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/planning.rs::collect_maintenance_evidence -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/planning.rs::collect_maintenance_evidence -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host.rs::PolicyHost::active_loaded_at -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host.rs::PolicyHost::recover_dispatches -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host.rs::PolicyHost::recover_planning_signals -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host.rs::PolicyHost::remedy_completed_run -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host/catalog_preview.rs::read_state -> query",
        2,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host/catalog_transaction.rs::CatalogStore::load_verified_active -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host/catalog_transaction.rs::CatalogStore::migrate_legacy_active_pointer -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host/catalog_transaction.rs::CatalogStore::migrate_legacy_active_pointer -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/policy_host/catalog_transaction.rs::CatalogStore::project_catalog_lineage -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/recovery_read.rs::read_pages -> query_page",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/suspension_report.rs::Snapshot::query -> query",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/suspension_report.rs::Snapshot::run_failure -> query",
        2,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
    (
        "crates/runtime-host/src/suspension_report.rs::suspension_report -> latest_sequence",
        1,
        "today's reader; AO-4 and AO-6 narrow the reads to X1 to X4, X6 and the views",
    ),
];

const CAPTURE_SITES: &[(&str, usize, &str)] = &[
    (
        "crates/runtime-host/src/host.rs::RuntimeHost::start_with_provider -> capture_handle",
        1,
        "the ledger hands the host its capture interface at start",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::capture_store_change_under_gate -> .ledger_capture",
        1,
        "capture point: the one holder of the capture interface",
    ),
    (
        "crates/runtime-host/src/host/facts.rs::HostShared::capture_store_change_under_gate -> capture",
        1,
        "capture point: the store's changes (G2) and their invalidations",
    ),
];

const TIER_SITES: &[(&str, usize, &str)] = &[];
