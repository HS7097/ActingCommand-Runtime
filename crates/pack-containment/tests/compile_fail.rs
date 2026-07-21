// SPDX-License-Identifier: AGPL-3.0-only

#[test]
fn loaded_bundle_cannot_be_constructed_outside_crate() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/loaded_bundle_construct.rs");
}

#[test]
fn admitted_package_cannot_be_constructed_outside_crate() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/admitted_package_construct.rs");
}

#[test]
fn raw_admission_documents_are_not_importable() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/raw_admission_document.rs");
}

#[test]
fn loaded_bundle_does_not_expose_entry_bytes() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/loaded_bundle_entry_bytes.rs");
}
