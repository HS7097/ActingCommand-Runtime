// SPDX-License-Identifier: AGPL-3.0-only

// Workflow #411 A11 (v0.12.1-rc.2; exec gate key 84; no register section): `actinglab resource
// validate --changed-paths-file` takes an absolute path to the NUL-delimited Git name-status list
// wherever the list is, for example in the Git directory of a linked worktree, outside the
// repository. The list is read as a bounded regular file with the same pair and status rules; it
// is no declaration, so it is neither in `read_files` nor counted in `read_bytes`. A missing
// absolute list is refused with `resource_declaration_invalid` naming the path as given. Every
// expected value comes from the A11 brief's requirements and contracts/resource-declarations.md.

use super::*;
use actingcommand_contract::codes::SharedCode;

const DECLARATION: &str = "declaration-exclusions.json";
const DECLARATION_BYTES: &[u8] =
    br#"{"schema_version":"actingcommand.declaration-exclusions.v1","directories":["regression"]}"#;

// A repository holding one valid declaration.
fn repository() -> TempDir {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join(DECLARATION), DECLARATION_BYTES).unwrap();
    temp
}

fn validate_with_list(repo: &Path, list: &Path) -> CliResult {
    run_cli(
        [
            "--json",
            "resource",
            "validate",
            "--repo",
            repo.to_str().unwrap(),
            "--changed-paths-file",
            list.to_str().unwrap(),
        ],
        true,
    )
}

// A list in a directory outside the repository, given by its absolute path, selects the
// declaration it names; the declaration is valid and is the only file read.
#[test]
fn a11_an_absolute_changed_paths_file_outside_the_repository_is_read() {
    let repo = repository();
    let outside = TempDir::new().unwrap();
    let list = outside.path().join("resource-changed-paths");
    fs::write(&list, format!("M\0{DECLARATION}\0")).unwrap();
    assert!(list.is_absolute());

    let result = validate_with_list(repo.path(), &list);
    assert_eq!(result.exit_code(), 0, "{}", result.envelope_json());
    let report = result.envelope.data.as_ref().unwrap();
    assert_eq!(report["status"], "valid", "{report}");
    assert_eq!(
        report["entries"],
        json!([{"path": DECLARATION, "status": "valid", "family": "declaration_exclusions"}]),
        "{report}"
    );
    assert_eq!(report["read_files"], json!([DECLARATION]), "{report}");
    assert_eq!(
        report["read_bytes"],
        json!(DECLARATION_BYTES.len()),
        "{report}"
    );
}

// An absolute list path that names nothing is refused, naming the path as given.
#[test]
fn a11_a_missing_absolute_changed_paths_file_is_refused_naming_it() {
    let repo = repository();
    let outside = TempDir::new().unwrap();
    let list = outside.path().join("missing-changed-paths");
    assert!(list.is_absolute());

    let result = validate_with_list(repo.path(), &list);
    assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
    let error = result.envelope.error.as_ref().unwrap();
    assert_eq!(error.code, SharedCode::ResourceDeclarationInvalid.as_str());
    assert_eq!(
        error.details.as_ref().unwrap()["declaration_file"],
        list.to_str().unwrap().replace('\\', "/"),
        "{}",
        result.envelope_json()
    );
}
