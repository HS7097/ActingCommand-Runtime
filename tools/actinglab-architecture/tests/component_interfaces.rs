// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #378 A8 (order-v012 §3.6; contracts/v012-register.md §6; model-378 §10 as amended
//! by §16 A-7): the Runtime's interface declaration `distribution/windows/component-interfaces.json`
//! declares each v0.12.0 revision once, and the Windows exact-SHA build validates every interface
//! the declaration names before it copies the declaration into the Runtime and Tools manifests.
//!
//! The ranges follow the bump rule of `contracts/component-interfaces.md`: a writer of a new
//! revision raises `max`, and `min` rises where v0.12.0 no longer reads the older revision (no
//! old-version compatibility, Alice 2026-10-11).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const DECLARATION: &str = "distribution/windows/component-interfaces.json";
const BUILD_WORKFLOW: &str = ".github/workflows/windows-remote-build.yml";
const SCHEMA_VERSION: &str = "actingcommand.component-interfaces.v1";

/// The v0.12.0 declaration, one row per interface:
/// - `actingd-config` 5 (register §5): the one revision v0.12.0 raises; the keys option 丙 and
///   NAV-3a removed are refused, so no revision-4 file is read. Workflow #411 B15 R3 (v0.12.3,
///   register §1.113): revision 6 adds the optional remedy keys; a revision-5 file is still read,
///   so [5, 6]. Workflow #406 T12 (register §1.136): revision 7 adds the optional `touch`
///   section; a revision-5 or revision-6 file is still read, so [5, 7]. #411 A38 (register
///   §1.148): revision 8 adds the optional `recovery_ladder.fence_recheck` member; revisions 5
///   to 7 are still read, so [5, 8].
/// - `install-control` stays [0, 1] (model-378 §16 A-7; U-4 raises it in 0.12.x).
/// - `install-selection` is unchanged (model-378 §10).
/// - `ledger` [3, 3]: SQLite format 2, `actingcommand.event.v3`; format 1 is refused.
/// - `package` 3: the one `package` bump of v0.12.0 (register §6); only bundle index v4 is read.
///   Workflow #411 B15 R1 (register §1.111) raises `max` to 4 in v0.12.3: a pack's `remedy` and
///   `provides`; a revision-3 pack is still read. Workflow #406 T3 (register §1.137) raises it to
///   5: the click `single_touch_path_drag_v1`; a revision-3 or revision-4 pack is still read.
/// - `runtime-client` [2, 2]: request v4, receipt v2; requests v1 to v3 are refused.
/// - `runtime-outcomes` [1, 1]: the new interface of model-378 §10.
const V0120: &[(&str, [u64; 2])] = &[
    ("actingd-config", [5, 8]),
    ("install-control", [0, 1]),
    ("install-selection", [1, 1]),
    ("ledger", [3, 3]),
    ("package", [3, 5]),
    ("runtime-client", [2, 2]),
    ("runtime-outcomes", [1, 1]),
];

#[test]
fn the_runtime_declares_each_v0120_interface_revision_once() {
    let declaration = read_declaration();
    assert_eq!(
        declaration
            .get("schema_version")
            .and_then(serde_json::Value::as_str),
        Some(SCHEMA_VERSION),
        "{DECLARATION}: schema_version"
    );
    let speaks = declaration
        .get("speaks")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("{DECLARATION}: `speaks` is an object"));
    let declared = speaks
        .iter()
        .map(|(name, range)| {
            let range = range
                .as_array()
                .and_then(|range| match range.as_slice() {
                    [min, max] => Some([min.as_u64()?, max.as_u64()?]),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{DECLARATION}: speaks.{name} is [min, max]"));
            (name.as_str(), range)
        })
        .collect::<Vec<_>>();
    let mut required = V0120.to_vec();
    required.sort_unstable();
    let mut declared = declared;
    declared.sort_unstable();
    assert_eq!(
        declared, required,
        "{DECLARATION} declares the v0.12.0 revisions of order-v012 §3.6 and register §6"
    );
}

#[test]
fn the_exact_sha_build_validates_every_declared_interface() {
    let workflow = fs::read_to_string(workspace_root().join(BUILD_WORKFLOW))
        .unwrap_or_else(|error| panic!("read {BUILD_WORKFLOW}: {error}"));
    // The validator is the one-line `foreach ($name in @('…', …))` that follows the line
    // reading the declaration (`$interfacesPath`).
    let validated = workflow
        .lines()
        .skip_while(|line| !line.contains("$interfacesPath = "))
        .find(|line| line.trim_start().starts_with("foreach ($name in @("))
        .unwrap_or_else(|| {
            panic!("{BUILD_WORKFLOW}: no validator loop after the declaration is read")
        })
        .split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let declared = read_declaration()
        .get("speaks")
        .and_then(serde_json::Value::as_object)
        .map(|speaks| speaks.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let required = V0120
        .iter()
        .map(|(name, _)| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        validated, required,
        "{BUILD_WORKFLOW} validates every v0.12.0 interface (model-378 §10: runtime-outcomes joins the loop)"
    );
    assert_eq!(
        validated, declared,
        "{BUILD_WORKFLOW} validates exactly the interfaces {DECLARATION} names"
    );
}

fn read_declaration() -> serde_json::Value {
    let path = workspace_root().join(DECLARATION);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("parse {DECLARATION}: {error}"))
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the architecture tool sits two levels below the workspace root")
        .to_path_buf()
}
