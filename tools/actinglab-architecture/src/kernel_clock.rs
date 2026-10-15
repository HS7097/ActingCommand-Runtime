// SPDX-License-Identifier: AGPL-3.0-only

//! Test plan E-3 (EXEC-I3, test plan v2.2 §3.10): the contained-task kernel reads its clock and
//! waits only through its runtime (`ContainedTaskRuntime::monotonic_now` and
//! `ContainedTaskRuntime::pause`), so one run never compares instants of two clocks.

use proc_macro2::Span;
use syn::visit::Visit;
use syn::{Item, Type};

use crate::ledger_owners::{item_attributes, production_items};
use crate::{collect_use_leaves, flatten_tokens, path_names, token_paths};

/// One direct read of the process clock, or one thread sleep, in production code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelClockFinding {
    pub line: usize,
    /// The function it is in: `Owner::name` for a trait or impl method, `name` for a free
    /// function, `None` outside any function.
    pub function: Option<String>,
    /// `Instant::now` or `thread::sleep`.
    pub token: &'static str,
}

/// The production code of one kernel module file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelClockScan {
    pub findings: Vec<KernelClockFinding>,
    /// The production child modules the file declares without a body (`mod name;`).
    pub modules: Vec<String>,
}

/// Reads the production items of one module file (items under `#[cfg(test)]` are dropped) for
/// `Instant::now` and `thread::sleep`, as paths, imports and macro tokens. A child module with a
/// `#[path]` attribute is not followed and fails the scan.
pub fn inspect_kernel_clock_reads(path: &str, source: &str) -> Result<KernelClockScan, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let items = production_items(&file.items)?;
    let mut modules = Vec::new();
    for item in &items {
        if let Item::Mod(module) = item
            && module.content.is_none()
        {
            if item_attributes(item)?
                .iter()
                .any(|attribute| attribute.path().is_ident("path"))
            {
                return Err(format!("{path}: module `{}` has a #[path]", module.ident));
            }
            modules.push(module.ident.to_string());
        }
    }
    let mut visitor = ClockVisitor {
        owner: None,
        function: None,
        findings: Vec::new(),
    };
    for item in &items {
        visitor.visit_item(item);
    }
    Ok(KernelClockScan {
        findings: visitor.findings,
        modules,
    })
}

struct ClockVisitor {
    owner: Option<String>,
    function: Option<String>,
    findings: Vec<KernelClockFinding>,
}

impl ClockVisitor {
    fn check(&mut self, segments: &[String], span: Span) {
        let names = segments.iter().map(String::as_str).collect::<Vec<_>>();
        let token = if names.windows(2).any(|pair| pair == ["Instant", "now"]) {
            "Instant::now"
        } else if names.windows(2).any(|pair| pair == ["thread", "sleep"]) {
            "thread::sleep"
        } else {
            return;
        };
        self.findings.push(KernelClockFinding {
            line: span.start().line,
            function: self.function.clone(),
            token,
        });
    }

    fn in_function(&mut self, name: String, visit: impl FnOnce(&mut Self)) {
        let qualified = match &self.owner {
            Some(owner) => format!("{owner}::{name}"),
            None => name,
        };
        let outer = self.function.replace(qualified);
        visit(self);
        self.function = outer;
    }

    fn in_owner(&mut self, owner: Option<String>, visit: impl FnOnce(&mut Self)) {
        let outer = std::mem::replace(&mut self.owner, owner);
        visit(self);
        self.owner = outer;
    }
}

impl<'ast> Visit<'ast> for ClockVisitor {
    fn visit_item_trait(&mut self, item: &'ast syn::ItemTrait) {
        self.in_owner(Some(item.ident.to_string()), |visitor| {
            syn::visit::visit_item_trait(visitor, item);
        });
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        let owner = match item.self_ty.as_ref() {
            Type::Path(path) => path
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string()),
            _ => None,
        };
        self.in_owner(owner, |visitor| syn::visit::visit_item_impl(visitor, item));
    }

    fn visit_item_fn(&mut self, item: &'ast syn::ItemFn) {
        // A free function nested in a method is not the method's.
        self.in_owner(None, |visitor| {
            visitor.in_function(item.sig.ident.to_string(), |visitor| {
                syn::visit::visit_item_fn(visitor, item);
            });
        });
    }

    fn visit_impl_item_fn(&mut self, item: &'ast syn::ImplItemFn) {
        self.in_function(item.sig.ident.to_string(), |visitor| {
            syn::visit::visit_impl_item_fn(visitor, item);
        });
    }

    fn visit_trait_item_fn(&mut self, item: &'ast syn::TraitItemFn) {
        self.in_function(item.sig.ident.to_string(), |visitor| {
            syn::visit::visit_trait_item_fn(visitor, item);
        });
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        let mut leaves = Vec::new();
        collect_use_leaves(&mut Vec::new(), &item.tree, &mut leaves);
        for (segments, span) in leaves {
            self.check(&segments, span);
        }
        syn::visit::visit_item_use(self, item);
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
