// SPDX-License-Identifier: AGPL-3.0-only

//! Workflow #384 D2-S2 (`rd-1009/d2/model-d2-fact-refresh.md` §3.2): the checks of the
//! configured recognition maintenance package bindings (`instances[].recognition_package`).
//!
//! - `check-config` prepares each bound package and refuses a binding that is no reader: its
//!   package cannot be read, does not qualify (check 1), or marks a key the configuration's
//!   `policy.facts` or a `ledger_fact` pool claims (checks 2-3). It warns `fact_reader_missing`
//!   for a realistic fact a catalog task reads that no reader marks.
//! - A Runtime start reads each bound package's marks without preparing it, runs without a
//!   binding it cannot read or whose marks are claimed (checks 2-3), and notes every such
//!   binding as `fact_reader_binding_ignored` in its `recognition_packages` subsystem row. The
//!   start goes on: a package's error is that package's alone.

use super::{PolicyBootstrap, RESOURCE_PACKAGE_DIRECTORY_DEADLINE, manifest};
use actingcommand_contract::outcome::{Code, Outcome};
use actingcommand_contract::{ConfigSubsystem, PackageRef, ReaderProblem};
use actingcommand_execution_kernel::{ContainedTaskError, PreparedContainedTask};
use actingcommand_runtime_host::{
    RuntimeHostConfig, recognition_binding_ignored, recognition_binding_invalid,
    recognition_binding_key_conflict, recognition_binding_qualification,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// How a check reads a bound package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindingRead {
    /// `check-config`: prepares the package and checks its qualification (checks 1-3).
    Prepare,
    /// A Runtime start: reads the package's marks without preparing it (checks 2-3).
    MarksOnly,
}

/// One configured binding as the check found it.
pub(crate) struct CheckedBinding {
    pub(crate) alias: String,
    pub(crate) path: PathBuf,
    /// The keys the reader's marks write, or the binding's refusal
    /// (`fact_reader_binding_invalid`).
    pub(crate) verdict: Result<Vec<String>, Outcome>,
}

/// Checks every configured binding, in alias order. The configured catalog is compiled again
/// for its pools; a catalog that does not compile has already refused the assembly.
pub(crate) fn check_bindings(
    configured: &BTreeMap<String, PathBuf>,
    host: &RuntimeHostConfig,
    policy: Option<&PolicyBootstrap>,
    read: BindingRead,
) -> Result<Vec<CheckedBinding>, Code> {
    if configured.is_empty() {
        return Ok(Vec::new());
    }
    let compiled = policy
        .map(|policy| actingcommand_policy::compile_catalog(&policy.catalog))
        .transpose()
        .map_err(|_| {
            actingcommand_contract::codes::SharedCode::PolicyCatalogCompileFailed.code()
        })?;
    let catalog = compiled.as_ref().map(|compiled| compiled.catalog());
    let policy_facts = host
        .policy_inputs()
        .map_or(&[][..], |inputs| inputs.facts().facts.as_slice());
    Ok(configured
        .iter()
        .map(|(alias, path)| {
            let verdict = read_binding(alias, path, read).and_then(|(package_id, keys)| {
                recognition_binding_key_conflict(
                    alias,
                    path,
                    &package_id,
                    &keys,
                    policy_facts,
                    catalog,
                )
                .map(|()| keys)
            });
            CheckedBinding {
                alias: alias.clone(),
                path: path.clone(),
                verdict,
            }
        })
        .collect())
}

/// The bound package's id and mark keys, or the binding's refusal: `package_unreadable` with
/// the loader's refusal as its cause, or (`Prepare` only) the package's disqualification.
fn read_binding(
    alias: &str,
    path: &Path,
    read: BindingRead,
) -> Result<(String, Vec<String>), Outcome> {
    let deadline = Instant::now() + RESOURCE_PACKAGE_DIRECTORY_DEADLINE;
    let unreadable = |error: ContainedTaskError| {
        recognition_binding_invalid(alias, path, ReaderProblem::PackageUnreadable)
            .caused_by(error.outcome())
    };
    match read {
        BindingRead::Prepare => {
            let descriptor =
                PreparedContainedTask::describe_path(alias, path, &PackageRef::default(), deadline)
                    .map_err(unreadable)?;
            recognition_binding_qualification(alias, path, &descriptor)?;
            Ok((
                descriptor.package_id().to_owned(),
                descriptor.reading_keys().to_vec(),
            ))
        }
        BindingRead::MarksOnly => {
            let marks = PreparedContainedTask::read_reading_marks(alias, path, deadline)
                .map_err(unreadable)?;
            Ok((marks.package_id, marks.fact_keys))
        }
    }
}

/// The keys each instance alias's reader marks, from bindings that passed.
pub(crate) fn marked_keys(checked: &[CheckedBinding]) -> BTreeMap<String, BTreeSet<String>> {
    checked
        .iter()
        .filter_map(|binding| {
            binding.verdict.as_ref().ok().map(|keys| {
                (
                    binding.alias.clone(),
                    keys.iter().cloned().collect::<BTreeSet<_>>(),
                )
            })
        })
        .collect()
}

/// `fact_reader_missing` for the configured catalog (D2 model §3.2): each realistic fact a
/// catalog task reads in an instance's scope that the instance's reader (`marked`) does not
/// mark. None without a policy.
pub(crate) fn missing_reader_warnings(
    policy: Option<&PolicyBootstrap>,
    marked: &BTreeMap<String, BTreeSet<String>>,
) -> Result<Vec<Outcome>, Code> {
    let Some(policy) = policy else {
        return Ok(Vec::new());
    };
    let compiled = actingcommand_policy::compile_catalog(&policy.catalog).map_err(|_| {
        actingcommand_contract::codes::SharedCode::PolicyCatalogCompileFailed.code()
    })?;
    Ok(actingcommand_runtime_host::missing_fact_readers(
        compiled.catalog(),
        marked,
    ))
}

/// A Runtime start's bindings: the locations it keeps, and the `recognition_packages`
/// subsystem row when the configuration names any binding, enabled when one is kept, with one
/// `note` link of `fact_reader_binding_ignored` per binding it runs without.
pub(crate) fn start_bindings(
    configured: &BTreeMap<String, PathBuf>,
    host: &RuntimeHostConfig,
    policy: Option<&PolicyBootstrap>,
) -> Result<(BTreeMap<String, PathBuf>, Option<ConfigSubsystem>), Code> {
    if configured.is_empty() {
        return Ok((BTreeMap::new(), None));
    }
    let mut kept = BTreeMap::new();
    let mut ignored = Vec::new();
    for binding in check_bindings(configured, host, policy, BindingRead::MarksOnly)? {
        match binding.verdict {
            Ok(_) => {
                kept.insert(binding.alias, binding.path);
            }
            Err(invalid) => {
                let package_id = invalid
                    .value("package_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                ignored.push(recognition_binding_ignored(
                    &binding.alias,
                    package_id.as_deref(),
                    invalid,
                ));
            }
        }
    }
    let subsystem = manifest::recognition_packages_subsystem(!kept.is_empty(), ignored);
    Ok((kept, Some(subsystem)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actingcommand_runtime_host::HostCode;

    // T14 (D2M:594; §3.2; 簿 L648), the start: a binding the start cannot use is left out and
    // noted in the `recognition_packages` row as `fact_reader_binding_ignored`, caused by its
    // `fact_reader_binding_invalid`; the start goes on. A configuration without a binding adds no
    // row.
    #[test]
    fn d2s2_t14_a_start_runs_without_a_binding_it_cannot_use_and_notes_it() {
        let root = tempfile::TempDir::new().expect("tempdir");
        let configured = BTreeMap::from([("node.a".to_owned(), root.path().join("missing"))]);
        let host = RuntimeHostConfig::new(root.path(), "0123456789abcdef");

        let (kept, subsystem) =
            start_bindings(&configured, &host, None).expect("the start goes on");

        assert!(kept.is_empty());
        let subsystem = subsystem.expect("the row of a configuration that binds a package");
        assert_eq!(subsystem.name, "recognition_packages");
        assert!(!subsystem.enabled);
        let [note] = subsystem.outcome.causes() else {
            panic!("one note: {:?}", subsystem.outcome);
        };
        assert_eq!(note.relation().as_str(), "note");
        let ignored = note.outcome();
        assert_eq!(
            ignored.code().as_str(),
            HostCode::FactReaderBindingIgnored.as_str()
        );
        assert_eq!(
            ignored.value("instance_id"),
            Some(&serde_json::json!("node.a"))
        );
        let [cause] = ignored.causes() else {
            panic!("one cause: {ignored:?}");
        };
        assert_eq!(cause.relation().as_str(), "caused_by");
        assert_eq!(
            cause.outcome().code().as_str(),
            HostCode::FactReaderBindingInvalid.as_str()
        );
        assert_eq!(
            cause.outcome().value("reader_problem"),
            Some(&serde_json::json!("package_unreadable"))
        );

        let (kept, subsystem) = start_bindings(&BTreeMap::new(), &host, None).expect("no binding");
        assert!(kept.is_empty());
        assert!(subsystem.is_none());
    }
}
