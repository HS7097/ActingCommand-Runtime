
/// Product variant (one-off, appended by the workflow): the return-home fallback is configured
/// for both games the same-facts packages use; a page-graph package never falls back.
fn same_config(root: &TempDir) -> RuntimeHostConfig {
    let mapped = "fixture01.l2c.return-home".to_owned();
    config(root)
        .with_prerequisite_packages(BTreeMap::from([(
            mapped.clone(),
            ContainedTaskRecoveryBinding::new(
                root.path().join("mapped-return-home.zip").display().to_string(),
                "8".repeat(64),
            )
            .expect("mapped binding"),
        )]))
        .with_return_home_packages(BTreeMap::from([
            (("fixture01".to_owned(), "test".to_owned()), mapped.clone()),
            (("neutral".to_owned(), "test".to_owned()), mapped),
        ]))
}
