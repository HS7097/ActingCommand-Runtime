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

/// Keeps the C3a read-only admission capability from becoming a writable authority escape.
pub fn inspect_readonly_capture_capability(
    path: &str,
    source: &str,
) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut found_struct = false;
    let mut found_impl = false;
    let mut violations = Vec::new();
    for item in &file.items {
        match item {
            Item::Struct(item_struct) if item_struct.ident == "ReadOnlyCaptureCapability" => {
                found_struct = true;
                if item_struct.fields.iter().any(|field| is_public(&field.vis)) {
                    violations.push(format!(
                        "{path}: ReadOnlyCaptureCapability exposes a public field"
                    ));
                }
            }
            Item::Impl(item_impl)
                if impl_self_ident(item_impl)
                    .is_some_and(|ident| ident == "ReadOnlyCaptureCapability") =>
            {
                found_impl = true;
                if let Some((_, trait_path, _)) = &item_impl.trait_ {
                    let trait_name = trait_path
                        .segments
                        .last()
                        .map(|segment| segment.ident.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    violations.push(format!(
                        "{path}: ReadOnlyCaptureCapability explicitly implements {trait_name}"
                    ));
                    continue;
                }
                for item in &item_impl.items {
                    let syn::ImplItem::Fn(method) = item else {
                        continue;
                    };
                    if is_public(&method.vis)
                        && !matches!(
                            method.sig.ident.to_string().as_str(),
                            "instance_id" | "recognition_id"
                        )
                    {
                        violations.push(format!(
                            "{path}: ReadOnlyCaptureCapability exposes unexpected public method {}",
                            method.sig.ident
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    if !found_struct {
        violations.push(format!("{path}: ReadOnlyCaptureCapability is missing"));
    }
    if !found_impl {
        violations.push(format!(
            "{path}: ReadOnlyCaptureCapability inherent implementation is missing"
        ));
    }
    Ok(violations)
}

/// Enforces the sole public global-ledger append ingress.
pub fn inspect_global_append_ingress(path: &str, source: &str) -> Result<Vec<String>, String> {
    let file = syn::parse_file(source).map_err(|err| format!("failed to parse {path}: {err}"))?;
    let mut append_methods = Vec::new();
    let mut alternate_ingress_methods = Vec::new();
    for item in &file.items {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        if impl_self_ident(item_impl).is_none_or(|ident| ident != "GlobalLedger") {
            continue;
        }
        for item in &item_impl.items {
            let syn::ImplItem::Fn(method) = item else {
                continue;
            };
            if method.sig.ident == "append" && is_public(&method.vis) {
                append_methods.push(method);
                continue;
            }
            if is_public(&method.vis)
                && (method.sig.ident.to_string().starts_with("append")
                    || method_accepts_event_ingress(method))
            {
                alternate_ingress_methods.push(method.sig.ident.to_string());
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
        let method = append_methods[0];
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
            && pattern_ident(&typed[0].pat).is_some_and(|ident| ident == "draft")
            && type_last_ident(&typed[0].ty).is_some_and(|ident| ident == "SanitizedEventDraft");
        if !exact {
            violations.push(format!(
                "{path}: GlobalLedger::append must accept exactly draft: SanitizedEventDraft"
            ));
        }
    }
    for method in alternate_ingress_methods {
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
    let mut found = false;
    for item in &file.items {
        match item {
            Item::Struct(item_struct) if item_struct.ident == "PersistedEvent" => {
                found = true;
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
                if impl_self_ident(item_impl).is_some_and(|ident| ident == "PersistedEvent") =>
            {
                if item_impl
                    .trait_
                    .as_ref()
                    .and_then(|(_, path, _)| path.segments.last())
                    .is_some_and(|segment| segment.ident == "Deserialize")
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
                        && signature_returns_ident(&method.sig, &["Self", "PersistedEvent"])
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
    if !found {
        violations.push(format!("{path}: missing public PersistedEvent definition"));
    }
    Ok(violations)
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
            Item::Struct(item_struct)
                if is_public(&item_struct.vis) && item_struct.ident == "ArtifactStoreIssuer" =>
            {
                violations.push(format!(
                    "{path}: public ArtifactStoreIssuer is forbidden until the real store boundary exists"
                ));
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
                    if local == "ArtifactStoreIssuer" || target == "ArtifactStoreIssuer" {
                        violations.push(format!("{path}: public use exposes ArtifactStoreIssuer"));
                    }
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
                        if externally_callable && returns_capability {
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

#[cfg(test)]
mod tests {
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
