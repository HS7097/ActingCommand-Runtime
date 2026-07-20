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
