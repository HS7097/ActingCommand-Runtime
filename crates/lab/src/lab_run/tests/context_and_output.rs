// SPDX-License-Identifier: AGPL-3.0-only

#[test]
fn manifest_entry_task_id_conflict_is_fatal() {
    let control = test_control();
    let manifest = json!({"entry_task_id": "other_task"});

    let err = validate_manifest_entry_task_id(Path::new("manifest.json"), &manifest, &control)
        .expect_err("conflict is fatal");

    assert_eq!(err.code, "package_invalid");
    assert!(err.message.contains("conflicts with control entry_task_id"));
}

#[test]
fn screenshot_names_are_timestamp_based_with_suffixes() {
    let temp = TempDir::new().expect("temp");
    let mut names =
        actingcommand_artifact_store::ScreenshotNameAllocator::new(temp.path()).expect("names");
    let time = 1_672_531_200_123;
    let first = names.allocate(time).expect("first name");
    let second = names.allocate(time).expect("second name");

    assert_eq!(first, "20230101000000123.png");
    assert_eq!(second, "20230101000000123-01.png");
}

#[test]
fn rejects_dangerous_zip_entry_without_writing_it() {
    let temp = TempDir::new().expect("temp");
    let zip = temp.path().join("input.zip");
    write_test_zip(
        &zip,
        &[
            ("control.json", br#"{}"#),
            ("resources/manifest.json", br#"{}"#),
            ("resources/tool.exe", b"danger"),
        ],
    );

    let err = match validate_lab_package_zip(&zip) {
        Ok(_) => panic!("dangerous entry accepted"),
        Err(err) => err,
    };

    assert_eq!(err.code, "package_invalid");
    assert!(
        !temp
            .path()
            .join("input")
            .join("resources")
            .join("tool.exe")
            .exists()
    );
}
