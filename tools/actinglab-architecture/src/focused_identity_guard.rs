// SPDX-License-Identifier: AGPL-3.0-only

use std::collections::{HashMap, HashSet};
use std::fmt;

use syn::parse::{Parse, ParseStream};
use syn::visit::{self, Visit};
use syn::{
    Attribute, BinOp, Block, Expr, ExprCall, ExprClosure, ExprForLoop, ExprIf, ExprMatch,
    ExprMethodCall, File, FnArg, ImplItem, Item, ItemConst, ItemFn, ItemImpl, ItemStatic, Local,
    Macro, Pat, Path, Signature, Token, Type,
};

pub const SCHEDULER_IDENTITY_DECLARATION_VERSION: &str = "issue75-c2-v1";
pub const SCHEDULER_IDENTITY_SOURCE_PATHS: &[&str] = &[
    "crates/actingcommand-contract/src/fact.rs",
    "crates/policy/src/evaluator.rs",
    "crates/policy/src/strategy.rs",
    "crates/runtime-host/src/policy_host.rs",
    "crates/execution-kernel/src/environment.rs",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerIdentityTarget {
    pub path: &'static str,
    pub owner: Option<&'static str>,
    pub function: &'static str,
}

const fn target(
    path: &'static str,
    owner: Option<&'static str>,
    function: &'static str,
) -> SchedulerIdentityTarget {
    SchedulerIdentityTarget {
        path,
        owner,
        function,
    }
}

pub const SCHEDULER_IDENTITY_TARGETS: &[SchedulerIdentityTarget] = &[
    target("crates/actingcommand-contract/src/fact.rs", Some("FactScope"), "matches"),
    target("crates/policy/src/evaluator.rs", None, "scope_matches_instance"),
    target("crates/policy/src/strategy.rs", None, "validate_assessment"),
    target("crates/policy/src/strategy.rs", None, "require_game_scope"),
    target(
        "crates/policy/src/strategy.rs",
        None,
        "require_game_predicate_scopes",
    ),
    target(
        "crates/runtime-host/src/policy_host.rs",
        None,
        "matching_activity_profile",
    ),
    target(
        "crates/execution-kernel/src/environment.rs",
        Some("EnvDetector"),
        "validate_scope",
    ),
    target(
        "crates/execution-kernel/src/environment.rs",
        Some("EnvironmentStateEngine"),
        "validate_result",
    ),
    target(
        "crates/execution-kernel/src/environment.rs",
        Some("EnvironmentStateEngine"),
        "validate_fact_snapshot_scope",
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerIdentityGuardErrorKind {
    SourceSet,
    Parse,
    MissingTarget,
    DuplicateTarget,
    TargetDrift,
    IdentityLiteral,
    IdentityConstant,
    BuiltInIdentity,
    UnsupportedControl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchedulerIdentityGuardError {
    pub kind: SchedulerIdentityGuardErrorKind,
    pub path: String,
    pub owner: Option<String>,
    pub function: Option<String>,
    pub message: String,
}

impl fmt::Display for SchedulerIdentityGuardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for SchedulerIdentityGuardError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerIdentityInspection {
    pub inspected_files: usize,
    pub inspected_targets: usize,
}

#[derive(Clone, Copy)]
struct ControlSpec {
    params: &'static [&'static str],
    roots: &'static [&'static str],
    identity: &'static [&'static str],
    comparisons: &'static [&'static str],
    ifs: &'static [&'static str],
    if_else: &'static [bool],
    matches: &'static [&'static str],
    macros: &'static [&'static str],
    allowed_identity_calls: &'static [&'static str],
    required_calls: &'static [(&'static str, usize)],
    required_call_heads: &'static [(&'static str, usize)],
    method_names: &'static [&'static str],
    required_methods: &'static [(&'static str, usize)],
}

const SPECS: &[ControlSpec] = &[
    spec(
        &["&self", "context:&InstanceFactContext"],
        &["self", "context"],
        &["instance_id", "&context.instance_id", "server_id", "&context.server_id", "game_id", "&context.game_id"],
        &["(instance_id==&context.instance_id)", "(server_id==&context.server_id)", "(game_id==&context.game_id)"],
        &[],
        &[],
        &["self=>Self::Instance{instance_id}[c:(instance_id==&context.instance_id);f:;m:;i:0;x:0]|Self::Server{server_id}[c:(server_id==&context.server_id);f:;m:;i:0;x:0]|Self::Game{game_id}[c:(game_id==&context.game_id);f:;m:;i:0;x:0]"],
        &[], &[], &[], &[], &[], &[],
    ),
    spec(
        &["scope:&ScopeSelector", "instance:&InstanceSnapshot"],
        &["scope", "instance"],
        &["instance_id", "&instance.instance_id", "server_id", "&instance.server_id", "game_id", "&instance.game_id"],
        &["(instance_id==&instance.instance_id)", "(server_id==&instance.server_id)", "(game_id==&instance.game_id)"],
        &[], &[],
        &["scope=>ScopeSelector::Instance{instance_id}[c:(instance_id==&instance.instance_id);f:;m:;i:0;x:0]|ScopeSelector::Server{server_id}[c:(server_id==&instance.server_id);f:;m:;i:0;x:0]|ScopeSelector::Game{game_id}[c:(game_id==&instance.game_id);f:;m:;i:0;x:0]"],
        &[], &[], &[], &[], &[], &[],
    ),
    spec(
        &["assessment:&StrategicInstanceAssessment", "report_game_id:&str"],
        &["assessment", "report_game_id"],
        &["assessment.game_id", "&assessment.game_id", "report_game_id"],
        &["(assessment.game_id!=report_game_id)", "(assessment.deadline_unix_ms==#int:0)", "(value>=capability.as_str())"],
        &["((assessment.game_id!=report_game_id)||(assessment.deadline_unix_ms==#int:0))", "(previous_capability.is_some_and(|value|(value>=capability.as_str()))||(!capabilities.insert(capability)))"],
        &[false, false], &[], &[],
        &["validate_identifier(&assessment.game_id,#str:assessment_game_id)"],
        &[], &[], &[], &[],
    ),
    spec(
        &["scope:&ScopeSelector", "game_id:&str", "kind:&str"],
        &["scope", "game_id"],
        &["scope", "value", "game_id"],
        &[],
        &["matches(scope,ScopeSelector::Game{game_id:value},(value==game_id))"],
        &[true], &[],
        &["format", "matches(scope,ScopeSelector::Game{game_id:value},(value==game_id))"],
        &[], &[],
        &[("Ok", 1), ("Err", 1), ("StrategyError::mismatch", 1)],
        &[], &[],
    ),
    spec(
        &["predicate:&PredicateSpec", "game_id:&str"],
        &["predicate", "game_id"],
        &["predicate", "scope", "game_id"],
        &[], &[], &[],
        &["predicate=>(PredicateSpec::All{predicates}|PredicateSpec::Any{predicates})[c:;f:require_game_predicate_scopes(predicate,game_id),Ok(());m:;i:0;x:0]|PredicateSpec::Not{predicate}[c:;f:require_game_predicate_scopes(predicate,game_id);m:;i:0;x:0]|(PredicateSpec::Fact{scope,..}|PredicateSpec::RecordDeadline{scope,..})[c:;f:require_game_scope(scope,game_id,#str:template fact predicate);m:;i:0;x:0]|(PredicateSpec::Clock{..}|PredicateSpec::ResourceProjection{..}|PredicateSpec::DependencyCompleted{..}|PredicateSpec::Outcome{..})[c:;f:Ok(());m:;i:0;x:0]"],
        &[],
        &["require_game_predicate_scopes(predicate,game_id)", "require_game_scope(scope,game_id,#str:template fact predicate)"],
        &[("require_game_predicate_scopes(predicate,game_id)", 2), ("require_game_scope(scope,game_id,#str:template fact predicate)", 1)],
        &[], &[], &[],
    ),
    spec(
        &["profiles:&[ActivityProfile]", "instance:&InstanceSnapshot"],
        &["profiles", "instance"],
        &["instance_id", "&instance.instance_id", "server_id", "&instance.server_id", "game_id", "&instance.game_id"],
        &["(instance_id==&instance.instance_id)", "(server_id==&instance.server_id)", "(game_id==&instance.game_id)"],
        &[], &[],
        &["&profile.scope=>ScopeSelector::Instance{instance_id}[c:(instance_id==&instance.instance_id);f:;m:;i:0;x:0]|ScopeSelector::Server{server_id}[c:(server_id==&instance.server_id);f:;m:;i:0;x:0]|ScopeSelector::Game{game_id}[c:(game_id==&instance.game_id);f:;m:;i:0;x:0]"],
        &[], &[], &[], &[],
        &["cmp", "cmp", "filter", "iter", "max_by", "then_with"],
        &[("scope_specificity(&left.scope).cmp(&scope_specificity(&right.scope))", 1), ("right.id.cmp(&left.id)", 1)],
    ),
    spec(
        &["&self", "game_id:&str", "server_id:&str"],
        &["self", "game_id", "server_id"],
        &["&self.game_id", "&self.server_id", "game", "game_id", "server", "server_id"],
        &["(game!=game_id)", "(server!=server_id)"],
        &["let Some(game)=&self.game_id", "(game!=game_id)", "(let Some(server)=&self.server_id&&(server!=server_id))"],
        &[false, false, false], &[], &["format", "format"],
        &["canonical_environment_game(game)"],
        &[("canonical_environment_game(game)", 1)],
        &[], &[], &[],
    ),
    spec(
        &["&self", "result:&EnvDetectionResult", "resource_hash:&str", "now_ms:u64"],
        &["self", "result", "resource_hash"],
        &["result.instance_id", "self.scope.instance_id", "result.game_id", "self.scope.game_id", "result.server_id", "self.scope.server_id", "result.detector_id", "self.detector.id", "result.resource_pack_id", "self.scope.resource_pack_id", "result.resource_pack_hash", "resource_hash"],
        &["(result.schema_version!=ENV_RESULT_SCHEMA_VERSION)", "(result.instance_id!=self.scope.instance_id)", "(result.game_id!=self.scope.game_id)", "(result.server_id!=self.scope.server_id)", "(result.detector_id!=self.detector.id)", "(result.detector_version!=self.detector.version)", "(result.resource_pack_id!=self.scope.resource_pack_id)", "(result.resource_pack_hash!=resource_hash)"],
        &["(result.schema_version!=ENV_RESULT_SCHEMA_VERSION)", "(result.instance_id!=self.scope.instance_id)", "((result.game_id!=self.scope.game_id)||(result.server_id!=self.scope.server_id))", "((result.detector_id!=self.detector.id)||(result.detector_version!=self.detector.version))", "((result.resource_pack_id!=self.scope.resource_pack_id)||(result.resource_pack_hash!=resource_hash))"],
        &[false, false, false, false, false], &[], &["format", "format", "format"],
        &[], &[], &[], &[], &[],
    ),
    spec(
        &["&self", "snapshot:&InstanceFactSnapshot"],
        &["self", "snapshot"],
        &["snapshot", "snapshot.context.instance_id", "self.scope.instance_id", "snapshot.context.game_id", "self.scope.game_id", "snapshot.context.server_id", "self.scope.server_id"],
        &["(snapshot.context.instance_id!=self.scope.instance_id)", "(snapshot.context.game_id!=self.scope.game_id)", "(snapshot.context.server_id!=self.scope.server_id)"],
        &["(snapshot.context.instance_id!=self.scope.instance_id)", "((snapshot.context.game_id!=self.scope.game_id)||(snapshot.context.server_id!=self.scope.server_id))"],
        &[false, false], &[], &[],
        &["snapshot.validate()"], &[], &[], &[],
        &[("snapshot.validate()", 1)],
    ),
];

const fn spec(
    params: &'static [&'static str],
    roots: &'static [&'static str],
    identity: &'static [&'static str],
    comparisons: &'static [&'static str],
    ifs: &'static [&'static str],
    if_else: &'static [bool],
    matches: &'static [&'static str],
    macros: &'static [&'static str],
    allowed_identity_calls: &'static [&'static str],
    required_calls: &'static [(&'static str, usize)],
    required_call_heads: &'static [(&'static str, usize)],
    method_names: &'static [&'static str],
    required_methods: &'static [(&'static str, usize)],
) -> ControlSpec {
    ControlSpec {
        params,
        roots,
        identity,
        comparisons,
        ifs,
        if_else,
        matches,
        macros,
        allowed_identity_calls,
        required_calls,
        required_call_heads,
        method_names,
        required_methods,
    }
}

type GuardResult<T> = Result<T, SchedulerIdentityGuardError>;

struct Context<'a> {
    target: &'a SchedulerIdentityTarget,
    spec: &'a ControlSpec,
}

struct LocatedFn<'a> {
    signature: &'a Signature,
    block: &'a Block,
    attrs: Vec<&'a Attribute>,
}

pub fn inspect_scheduler_identity_controls(
    sources: &[(&str, &str)],
) -> GuardResult<SchedulerIdentityInspection> {
    if SPECS.len() != SCHEDULER_IDENTITY_TARGETS.len() {
        return Err(source_error("internal target/spec count drift"));
    }
    let mut source_map = HashMap::new();
    for &(path, source) in sources {
        if !SCHEDULER_IDENTITY_SOURCE_PATHS.contains(&path) {
            return Err(source_error(format!("unexpected source {path}")));
        }
        if source_map.insert(path, source).is_some() {
            return Err(source_error(format!("duplicate source {path}")));
        }
    }
    let mut parsed = HashMap::new();
    for path in SCHEDULER_IDENTITY_SOURCE_PATHS {
        let source = source_map
            .get(path)
            .ok_or_else(|| source_error(format!("missing source {path}")))?;
        parsed.insert(
            *path,
            syn::parse_file(source).map_err(|error| SchedulerIdentityGuardError {
                kind: SchedulerIdentityGuardErrorKind::Parse,
                path: (*path).to_owned(),
                owner: None,
                function: None,
                message: format!("source does not parse: {error}"),
            })?,
        );
    }
    for (index, target) in SCHEDULER_IDENTITY_TARGETS.iter().enumerate() {
        let context = Context {
            target,
            spec: SPECS
                .get(index)
                .ok_or_else(|| source_error("internal target/spec lookup drift"))?,
        };
        inspect_target(
            &context,
            parsed
                .get(target.path)
                .ok_or_else(|| source_error(format!("missing parsed source {}", target.path)))?,
        )?;
    }
    Ok(SchedulerIdentityInspection {
        inspected_files: parsed.len(),
        inspected_targets: SCHEDULER_IDENTITY_TARGETS.len(),
    })
}

fn inspect_target(context: &Context<'_>, file: &File) -> GuardResult<()> {
    let function = locate_target(context, file)?;
    if function
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("cfg") || attr.path().is_ident("cfg_attr"))
    {
        return Err(context.error(
            SchedulerIdentityGuardErrorKind::TargetDrift,
            "target or owning impl has cfg/cfg_attr drift",
        ));
    }
    let params = function
        .signature
        .inputs
        .iter()
        .map(parameter_key)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|message| {
            context.error(SchedulerIdentityGuardErrorKind::TargetDrift, message)
        })?;
    if params != context.spec.params {
        return Err(context.error(
            SchedulerIdentityGuardErrorKind::TargetDrift,
            format!(
                "parameter drift: expected {:?}, found {params:?}",
                context.spec.params
            ),
        ));
    }

    let mut controls = Controls::default();
    controls.visit_block(function.block);
    if let Some(error) = controls.errors.first() {
        return Err(context.error(
            SchedulerIdentityGuardErrorKind::UnsupportedControl,
            error,
        ));
    }
    if let Some(root) = controls
        .bindings
        .iter()
        .find(|root| context.spec.roots.contains(&root.as_str()))
    {
        return Err(context.error(
            SchedulerIdentityGuardErrorKind::UnsupportedControl,
            format!("identity source root {root} is shadowed"),
        ));
    }
    if controls.whiles != 0 {
        return Err(context.error(
            SchedulerIdentityGuardErrorKind::UnsupportedControl,
            "while/while-let control is not declared",
        ));
    }
    let snapshot = snapshot(context, &controls)?;
    if snapshot.comparisons != context.spec.comparisons
        || snapshot.ifs != context.spec.ifs
        || snapshot.if_else != context.spec.if_else
        || snapshot.matches != context.spec.matches
        || snapshot.macros != context.spec.macros
    {
        return Err(context.error(
            mismatch_kind(context, &controls),
            format!(
                "required owner controls drifted: comparisons={:?}, ifs={:?}, if_else={:?}, matches={:?}, macros={:?}",
                snapshot.comparisons,
                snapshot.ifs,
                snapshot.if_else,
                snapshot.matches,
                snapshot.macros
            ),
        ));
    }
    check_calls(context, &controls)?;
    check_methods(context, &controls)
}

struct Snapshot {
    comparisons: Vec<String>,
    ifs: Vec<String>,
    if_else: Vec<bool>,
    matches: Vec<String>,
    macros: Vec<String>,
}

fn snapshot(context: &Context<'_>, controls: &Controls<'_>) -> GuardResult<Snapshot> {
    let map = |result: Result<String, String>| {
        result.map_err(|message| {
            context.error(SchedulerIdentityGuardErrorKind::UnsupportedControl, message)
        })
    };
    let comparisons = controls
        .comparisons
        .iter()
        .map(|value| map(expr_shape(value)))
        .collect::<GuardResult<Vec<_>>>()?;
    let ifs = controls
        .ifs
        .iter()
        .map(|value| map(expr_shape(&value.cond)))
        .collect::<GuardResult<Vec<_>>>()?;
    let matches = controls
        .matches
        .iter()
        .map(|value| map(match_shape(value)))
        .collect::<GuardResult<Vec<_>>>()?;
    let mut macros = controls
        .macros
        .iter()
        .map(|value| map(macro_shape(value)))
        .collect::<GuardResult<Vec<_>>>()?;
    macros.sort();
    Ok(Snapshot {
        comparisons,
        ifs,
        if_else: controls
            .ifs
            .iter()
            .map(|value| value.else_branch.is_some())
            .collect(),
        matches,
        macros,
    })
}

fn check_calls(context: &Context<'_>, controls: &Controls<'_>) -> GuardResult<()> {
    let call_shapes = controls
        .calls
        .iter()
        .map(|call| expr_shape(&Expr::Call((**call).clone())))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|message| {
            context.error(SchedulerIdentityGuardErrorKind::UnsupportedControl, message)
        })?;
    for call in &controls.calls {
        if call
            .args
            .iter()
            .any(|arg| direct_identity(arg, context.spec.identity))
        {
            let shape = expr_shape(&Expr::Call((**call).clone())).map_err(|message| {
                context.error(SchedulerIdentityGuardErrorKind::UnsupportedControl, message)
            })?;
            if !context.spec.allowed_identity_calls.contains(&shape.as_str()) {
                return Err(context.error(
                    SchedulerIdentityGuardErrorKind::UnsupportedControl,
                    format!("undeclared identity helper call {shape}"),
                ));
            }
        }
    }
    for (required, count) in context.spec.required_calls {
        if call_shapes.iter().filter(|value| value == required).count() != *count {
            return Err(context.error(
                SchedulerIdentityGuardErrorKind::UnsupportedControl,
                format!("required call {required} must occur exactly {count} time(s)"),
            ));
        }
    }
    for (required, count) in context.spec.required_call_heads {
        if controls
            .calls
            .iter()
            .filter_map(|call| call_head(&call.func).ok())
            .filter(|value| value == required)
            .count()
            != *count
        {
            return Err(context.error(
                SchedulerIdentityGuardErrorKind::UnsupportedControl,
                format!("required call head {required} must occur exactly {count} time(s)"),
            ));
        }
    }
    Ok(())
}

fn check_methods(context: &Context<'_>, controls: &Controls<'_>) -> GuardResult<()> {
    for method in &controls.methods {
        if direct_identity(&method.receiver, context.spec.identity)
            || method
                .args
                .iter()
                .any(|arg| direct_identity(arg, context.spec.identity))
        {
            let shape = expr_shape(&Expr::MethodCall((**method).clone())).map_err(|message| {
                context.error(SchedulerIdentityGuardErrorKind::UnsupportedControl, message)
            })?;
            if !context.spec.allowed_identity_calls.contains(&shape.as_str()) {
                return Err(context.error(
                    SchedulerIdentityGuardErrorKind::UnsupportedControl,
                    format!("undeclared identity method call {shape}"),
                ));
            }
        }
    }
    if !context.spec.method_names.is_empty() {
        let mut names = controls
            .methods
            .iter()
            .map(|value| value.method.to_string())
            .collect::<Vec<_>>();
        names.sort();
        if names != context.spec.method_names {
            return Err(context.error(
                SchedulerIdentityGuardErrorKind::UnsupportedControl,
                format!("required method structure drifted: {names:?}"),
            ));
        }
    }
    for (required, count) in context.spec.required_methods {
        let actual = controls
            .methods
            .iter()
            .filter_map(|method| expr_shape(&Expr::MethodCall((**method).clone())).ok())
            .filter(|value| value == required)
            .count();
        if actual != *count {
            return Err(context.error(
                SchedulerIdentityGuardErrorKind::UnsupportedControl,
                format!("required method {required} must occur exactly {count} time(s)"),
            ));
        }
    }
    Ok(())
}

fn mismatch_kind(context: &Context<'_>, controls: &Controls<'_>) -> SchedulerIdentityGuardErrorKind {
    for comparison in &controls.comparisons {
        if let Expr::Binary(value) = comparison {
            let left = expr_shape(&value.left).ok();
            let right = expr_shape(&value.right).ok();
            if left
                .as_deref()
                .is_some_and(|value| context.spec.identity.contains(&value))
            {
                if let Some(kind) = candidate_kind(right.as_deref(), &controls.constants) {
                    return kind;
                }
            }
            if right
                .as_deref()
                .is_some_and(|value| context.spec.identity.contains(&value))
            {
                if let Some(kind) = candidate_kind(left.as_deref(), &controls.constants) {
                    return kind;
                }
            }
        }
    }
    for value in &controls.macros {
        if value.path.segments.last().is_some_and(|part| part.ident == "matches")
            && let Ok(input) = syn::parse2::<MatchesInput>(value.tokens.clone())
            && let Some(guard) = input.guard
            && let Expr::Binary(binary) = guard
        {
            let left = expr_shape(&binary.left).ok();
            let right = expr_shape(&binary.right).ok();
            if left
                .as_deref()
                .is_some_and(|value| context.spec.identity.contains(&value))
                && let Some(kind) = candidate_kind(right.as_deref(), &controls.constants)
            {
                return kind;
            }
        }
    }
    SchedulerIdentityGuardErrorKind::UnsupportedControl
}

fn candidate_kind(
    candidate: Option<&str>,
    constants: &HashSet<String>,
) -> Option<SchedulerIdentityGuardErrorKind> {
    let candidate = candidate?;
    if candidate.starts_with("#str:") || candidate.starts_with("#int:") {
        Some(SchedulerIdentityGuardErrorKind::IdentityLiteral)
    } else if constants.contains(candidate) {
        Some(SchedulerIdentityGuardErrorKind::IdentityConstant)
    } else if candidate.contains("::") {
        Some(SchedulerIdentityGuardErrorKind::BuiltInIdentity)
    } else {
        None
    }
}

fn direct_identity(expr: &Expr, identity: &[&str]) -> bool {
    expr_shape(expr)
        .ok()
        .is_some_and(|value| identity.contains(&value.as_str()))
}

fn locate_target<'a>(context: &Context<'_>, file: &'a File) -> GuardResult<LocatedFn<'a>> {
    let mut found = Vec::new();
    for item in &file.items {
        match (context.target.owner, item) {
            (None, Item::Fn(function)) if function.sig.ident == context.target.function => {
                found.push(LocatedFn {
                    signature: &function.sig,
                    block: &function.block,
                    attrs: function.attrs.iter().collect(),
                });
            }
            (Some(owner), Item::Impl(item_impl))
                if item_impl.trait_.is_none() && exact_owner(item_impl, owner) =>
            {
                for item in &item_impl.items {
                    if let ImplItem::Fn(function) = item
                        && function.sig.ident == context.target.function
                    {
                        let mut attrs = item_impl.attrs.iter().collect::<Vec<_>>();
                        attrs.extend(&function.attrs);
                        found.push(LocatedFn {
                            signature: &function.sig,
                            block: &function.block,
                            attrs,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    match found.len() {
        0 => Err(context.error(
            SchedulerIdentityGuardErrorKind::MissingTarget,
            "exact owner/function target is missing or moved",
        )),
        1 => found.pop().ok_or_else(|| {
            context.error(
                SchedulerIdentityGuardErrorKind::MissingTarget,
                "exact target lookup was empty",
            )
        }),
        count => Err(context.error(
            SchedulerIdentityGuardErrorKind::DuplicateTarget,
            format!("exact owner/function target occurs {count} times"),
        )),
    }
}

fn exact_owner(item_impl: &ItemImpl, owner: &str) -> bool {
    matches!(
        item_impl.self_ty.as_ref(),
        Type::Path(value)
            if value.qself.is_none()
                && value.path.segments.len() == 1
                && value.path.is_ident(owner)
    )
}

impl Context<'_> {
    fn error(
        &self,
        kind: SchedulerIdentityGuardErrorKind,
        message: impl Into<String>,
    ) -> SchedulerIdentityGuardError {
        SchedulerIdentityGuardError {
            kind,
            path: self.target.path.to_owned(),
            owner: self.target.owner.map(str::to_owned),
            function: Some(self.target.function.to_owned()),
            message: message.into(),
        }
    }
}

fn source_error(message: impl Into<String>) -> SchedulerIdentityGuardError {
    SchedulerIdentityGuardError {
        kind: SchedulerIdentityGuardErrorKind::SourceSet,
        path: "<source-set>".to_owned(),
        owner: None,
        function: None,
        message: message.into(),
    }
}

#[derive(Default)]
struct Controls<'ast> {
    comparisons: Vec<&'ast Expr>,
    ifs: Vec<&'ast ExprIf>,
    matches: Vec<&'ast ExprMatch>,
    calls: Vec<&'ast ExprCall>,
    methods: Vec<&'ast ExprMethodCall>,
    macros: Vec<&'ast Macro>,
    bindings: Vec<String>,
    constants: HashSet<String>,
    whiles: usize,
    errors: Vec<String>,
}

impl<'ast> Visit<'ast> for Controls<'ast> {
    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if comparison_op(&node.op).is_some() {
            self.comparisons.push(node.left.as_ref());
            self.comparisons.pop();
            self.comparisons.push(unsafe { &*(node as *const syn::ExprBinary as *const Expr) });
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        self.ifs.push(node);
        visit::visit_expr_if(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast ExprMatch) {
        self.matches.push(node);
        visit::visit_expr_match(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        self.calls.push(node);
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        self.methods.push(node);
        visit::visit_expr_method_call(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        self.macros.push(node);
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.whiles += 1;
        visit::visit_expr_while(self, node);
    }

    fn visit_local(&mut self, node: &'ast Local) {
        collect_bindings(&node.pat, &mut self.bindings);
        visit::visit_local(self, node);
    }

    fn visit_expr_closure(&mut self, node: &'ast ExprClosure) {
        for input in &node.inputs {
            collect_bindings(input, &mut self.bindings);
        }
        visit::visit_expr_closure(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast ExprForLoop) {
        collect_bindings(&node.pat, &mut self.bindings);
        visit::visit_expr_for_loop(self, node);
    }

    fn visit_item_const(&mut self, node: &'ast ItemConst) {
        self.constants.insert(node.ident.to_string());
    }

    fn visit_item_static(&mut self, node: &'ast ItemStatic) {
        self.constants.insert(node.ident.to_string());
    }

    fn visit_item_fn(&mut self, _node: &'ast ItemFn) {}

    fn visit_item_impl(&mut self, _node: &'ast ItemImpl) {}
}

fn collect_bindings(pat: &Pat, output: &mut Vec<String>) {
    match pat {
        Pat::Ident(value) => output.push(value.ident.to_string()),
        Pat::Reference(value) => collect_bindings(&value.pat, output),
        Pat::Slice(value) => value
            .elems
            .iter()
            .for_each(|item| collect_bindings(item, output)),
        Pat::Struct(value) => value
            .fields
            .iter()
            .for_each(|field| collect_bindings(&field.pat, output)),
        Pat::Tuple(value) => value
            .elems
            .iter()
            .for_each(|item| collect_bindings(item, output)),
        Pat::TupleStruct(value) => value
            .elems
            .iter()
            .for_each(|item| collect_bindings(item, output)),
        Pat::Type(value) => collect_bindings(&value.pat, output),
        Pat::Or(value) => value
            .cases
            .iter()
            .for_each(|item| collect_bindings(item, output)),
        _ => {}
    }
}

fn match_shape(value: &ExprMatch) -> Result<String, String> {
    let mut arms = Vec::new();
    for arm in &value.arms {
        let mut controls = Controls::default();
        controls.visit_expr(&arm.body);
        let comparisons = controls
            .comparisons
            .iter()
            .map(|value| expr_shape(value))
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        let calls = controls
            .calls
            .iter()
            .map(|value| expr_shape(&Expr::Call((**value).clone())))
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        let methods = controls
            .methods
            .iter()
            .map(|value| expr_shape(&Expr::MethodCall((**value).clone())))
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        arms.push(format!(
            "{}[c:{comparisons};f:{calls};m:{methods};i:{};x:{}]",
            pat_shape(&arm.pat)?,
            controls.ifs.len(),
            controls.matches.len()
        ));
    }
    Ok(format!("{}=>{}", expr_shape(&value.expr)?, arms.join("|")))
}

fn macro_shape(value: &Macro) -> Result<String, String> {
    let path = path_shape(&value.path)?;
    if value
        .path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "matches")
    {
        let parsed = syn::parse2::<MatchesInput>(value.tokens.clone())
            .map_err(|error| format!("unsupported matches! control: {error}"))?;
        let guard = parsed
            .guard
            .as_ref()
            .ok_or_else(|| "matches! identity guard is missing".to_owned())?;
        Ok(format!(
            "{path}({},{},{})",
            expr_shape(&parsed.expr)?,
            pat_shape(&parsed.pat)?,
            expr_shape(guard)?
        ))
    } else {
        Ok(path)
    }
}

struct MatchesInput {
    expr: Expr,
    pat: Pat,
    guard: Option<Expr>,
}

impl Parse for MatchesInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let pat = Pat::parse_multi_with_leading_vert(input)?;
        let guard = if input.peek(Token![if]) {
            input.parse::<Token![if]>()?;
            Some(input.parse()?)
        } else {
            None
        };
        if !input.is_empty() {
            return Err(input.error("trailing matches! tokens"));
        }
        Ok(Self { expr, pat, guard })
    }
}

fn expr_shape(value: &Expr) -> Result<String, String> {
    match value {
        Expr::Path(path) if path.qself.is_none() => path_shape(&path.path),
        Expr::Field(field) => {
            let syn::Member::Named(member) = &field.member else {
                return Err("tuple-field control is unsupported".to_owned());
            };
            Ok(format!("{}.{}", expr_shape(&field.base)?, member))
        }
        Expr::Reference(value) if value.mutability.is_none() => {
            Ok(format!("&{}", expr_shape(&value.expr)?))
        }
        Expr::Binary(value) => Ok(format!(
            "({}{}{})",
            expr_shape(&value.left)?,
            binary_op(&value.op).ok_or_else(|| "unsupported binary control".to_owned())?,
            expr_shape(&value.right)?
        )),
        Expr::Let(value) => Ok(format!(
            "let {}={}",
            pat_shape(&value.pat)?,
            expr_shape(&value.expr)?
        )),
        Expr::Call(value) => Ok(format!(
            "{}({})",
            expr_shape(&value.func)?,
            value
                .args
                .iter()
                .map(expr_shape)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        Expr::MethodCall(value) if value.turbofish.is_none() => Ok(format!(
            "{}.{}({})",
            expr_shape(&value.receiver)?,
            value.method,
            value
                .args
                .iter()
                .map(expr_shape)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        Expr::Closure(value) => Ok(format!(
            "|{}|{}",
            value
                .inputs
                .iter()
                .map(pat_shape)
                .collect::<Result<Vec<_>, _>>()?
                .join(","),
            expr_shape(&value.body)?
        )),
        Expr::Unary(value) => Ok(format!(
            "({}{})",
            match value.op {
                syn::UnOp::Not(_) => "!",
                syn::UnOp::Neg(_) => "-",
                syn::UnOp::Deref(_) => "*",
                _ => return Err("unsupported unary control".to_owned()),
            },
            expr_shape(&value.expr)?
        )),
        Expr::Lit(value) => match &value.lit {
            syn::Lit::Str(value) => Ok(format!("#str:{}", value.value())),
            syn::Lit::Int(value) => Ok(format!("#int:{}", value.base10_digits())),
            syn::Lit::Bool(value) => Ok(format!("#bool:{}", value.value)),
            _ => Err("unsupported literal control".to_owned()),
        },
        Expr::Tuple(value) => Ok(format!(
            "({})",
            value
                .elems
                .iter()
                .map(expr_shape)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        Expr::Paren(value) => Ok(format!("({})", expr_shape(&value.expr)?)),
        Expr::Group(value) => Ok(format!("group({})", expr_shape(&value.expr)?)),
        Expr::Macro(value) => macro_shape(&value.mac),
        _ => Err("unsupported direct owner control expression".to_owned()),
    }
}

fn pat_shape(value: &Pat) -> Result<String, String> {
    match value {
        Pat::Ident(value) if value.by_ref.is_none() && value.mutability.is_none() => {
            if let Some((_, subpat)) = &value.subpat {
                Ok(format!("{}@{}", value.ident, pat_shape(subpat)?))
            } else {
                Ok(value.ident.to_string())
            }
        }
        Pat::Wild(_) => Ok("_".to_owned()),
        Pat::Struct(value) => {
            let mut fields = value
                .fields
                .iter()
                .map(|field| {
                    let syn::Member::Named(member) = &field.member else {
                        return Err("tuple member in control pattern".to_owned());
                    };
                    let nested = pat_shape(&field.pat)?;
                    Ok(if nested == member.to_string() {
                        nested
                    } else {
                        format!("{member}:{nested}")
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            if value.rest.is_some() {
                fields.push("..".to_owned());
            }
            Ok(format!("{}{{{}}}", path_shape(&value.path)?, fields.join(",")))
        }
        Pat::TupleStruct(value) => Ok(format!(
            "{}({})",
            path_shape(&value.path)?,
            value
                .elems
                .iter()
                .map(pat_shape)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        )),
        Pat::Or(value) => Ok(format!(
            "({})",
            value
                .cases
                .iter()
                .map(pat_shape)
                .collect::<Result<Vec<_>, _>>()?
                .join("|")
        )),
        Pat::Reference(value) if value.mutability.is_none() => {
            Ok(format!("&{}", pat_shape(&value.pat)?))
        }
        _ => Err("unsupported direct owner control pattern".to_owned()),
    }
}

fn parameter_key(input: &FnArg) -> Result<String, String> {
    match input {
        FnArg::Receiver(value)
            if value.reference.is_some()
                && value.mutability.is_none()
                && value.colon_token.is_none() =>
        {
            Ok("&self".to_owned())
        }
        FnArg::Typed(value) => {
            let Pat::Ident(name) = value.pat.as_ref() else {
                return Err("target parameter is not a direct identifier".to_owned());
            };
            Ok(format!("{}:{}", name.ident, type_shape(&value.ty)?))
        }
        _ => Err("unsupported target receiver".to_owned()),
    }
}

fn type_shape(value: &Type) -> Result<String, String> {
    match value {
        Type::Reference(value) if value.mutability.is_none() => {
            Ok(format!("&{}", type_shape(&value.elem)?))
        }
        Type::Slice(value) => Ok(format!("[{}]", type_shape(&value.elem)?)),
        Type::Path(value) if value.qself.is_none() => path_shape(&value.path),
        _ => Err("unsupported target parameter type".to_owned()),
    }
}

fn path_shape(value: &Path) -> Result<String, String> {
    if value.leading_colon.is_some()
        || value
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, syn::PathArguments::None))
    {
        return Err("qualified/generic control path is unsupported".to_owned());
    }
    Ok(value
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::"))
}

fn call_head(value: &Expr) -> Result<String, String> {
    match value {
        Expr::Path(value) if value.qself.is_none() => path_shape(&value.path),
        _ => Err("unsupported call head".to_owned()),
    }
}

fn comparison_op(value: &BinOp) -> Option<&'static str> {
    match value {
        BinOp::Eq(_) => Some("=="),
        BinOp::Ne(_) => Some("!="),
        BinOp::Lt(_) => Some("<"),
        BinOp::Le(_) => Some("<="),
        BinOp::Gt(_) => Some(">"),
        BinOp::Ge(_) => Some(">="),
        _ => None,
    }
}

fn binary_op(value: &BinOp) -> Option<&'static str> {
    comparison_op(value).or_else(|| match value {
        BinOp::And(_) => Some("&&"),
        BinOp::Or(_) => Some("||"),
        _ => None,
    })
}
