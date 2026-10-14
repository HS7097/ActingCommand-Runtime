// Workflow #396 TOOL-2 (forced compromise T-04): `resource validate` accepts the regression case
// families a repository keeps in the directories its root `declaration-exclusions.json` names,
// in place of the "no mapped parser" refusal, and still refuses unknown JSON elsewhere. A mapped
// declaration family stays validated inside such a directory, and a declaration that does not
// parse is refused, never read as "nothing excluded".
#[test]
fn resource_validate_excludes_declared_regression_cases_and_refuses_unknown_json_elsewhere() {
    fn validate(repo: &Path, changed: &[&str]) -> CliResult {
        let mut args = vec![
            "--json".to_owned(),
            "resource".to_owned(),
            "validate".to_owned(),
            "--repo".to_owned(),
            repo.to_str().unwrap().to_owned(),
        ];
        for path in changed {
            args.push("--changed-path".to_owned());
            args.push((*path).to_owned());
        }
        run_cli(args, true)
    }
    fn refused(result: &CliResult, declaration_file: &str) {
        assert_eq!(result.exit_code(), 2, "{}", result.envelope_json());
        let error = result.envelope.error.as_ref().unwrap();
        assert_eq!(error.code, "resource_declaration_invalid");
        assert_eq!(
            error.details.as_ref().unwrap()["declaration_file"],
            declaration_file,
            "{}",
            result.envelope_json()
        );
    }

    let temp = TempDir::new().unwrap();
    let repo = temp.path();
    let declaration = "declaration-exclusions.json";
    let case = "regression/group/case-1/case.json";
    let grouped_case = "regression/scheduling/case.json";
    let unknown = "notes/extra.json";
    let mapped = "regression/group/case-1/broken.pack.json";
    for (path, bytes) in [
        (
            case,
            br#"{"schema_version":"repository.case.v1","expect":{"observe":{"page":null}}}"#
                .as_slice(),
        ),
        (grouped_case, br#"{"expect":{}}"#.as_slice()),
        (unknown, br#"{}"#.as_slice()),
        (mapped, br#"{}"#.as_slice()),
    ] {
        let file = repo.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
    }

    // Without the declaration a case file has no mapped parser.
    refused(&validate(repo, &[case]), case);

    fs::write(
        repo.join(declaration),
        br#"{"schema_version":"actingcommand.declaration-exclusions.v1","directories":["regression"]}"#,
    )
    .unwrap();
    let accepted = validate(repo, &[case, grouped_case, declaration]);
    assert_eq!(accepted.exit_code(), 0, "{}", accepted.envelope_json());
    let report = accepted.envelope.data.as_ref().unwrap();
    assert_eq!(report["status"], "valid");
    assert_eq!(
        report["entries"],
        json!([
            {"path": declaration, "status": "valid", "family": "declaration_exclusions"},
            {"path": case, "status": "excluded", "reason": "repository_declared_exclusion"},
            {"path": grouped_case, "status": "excluded", "reason": "repository_declared_exclusion"},
        ])
    );

    // A Git deletion inside a declared directory is excluded as well.
    let removed = "regression/group/case-2/case.json";
    fs::write(repo.join("changed.list"), format!("D\0{removed}\0")).unwrap();
    let removal = run_cli(
        [
            "--json",
            "resource",
            "validate",
            "--repo",
            repo.to_str().unwrap(),
            "--changed-paths-file",
            "changed.list",
        ],
        true,
    );
    assert_eq!(removal.exit_code(), 0, "{}", removal.envelope_json());
    assert_eq!(
        removal.envelope.data.as_ref().unwrap()["entries"],
        json!([{"path": removed, "status": "excluded", "reason": "repository_declared_exclusion"}])
    );

    // Unknown JSON outside the declared directories is still refused, and a mapped declaration
    // family inside one is still validated.
    refused(&validate(repo, &[unknown]), unknown);
    refused(&validate(repo, &[mapped]), mapped);

    // A declaration that does not parse refuses the case and names itself.
    fs::write(
        repo.join(declaration),
        br#"{"schema_version":"actingcommand.declaration-exclusions.v1","directories":["regression/"]}"#,
    )
    .unwrap();
    refused(&validate(repo, &[case]), declaration);
}
