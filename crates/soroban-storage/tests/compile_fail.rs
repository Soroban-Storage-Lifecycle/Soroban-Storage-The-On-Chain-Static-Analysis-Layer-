//! Regression tests for the `storage!` macro's compile-time guarantees:
//! critical data in temporary storage, and entries without a lifecycle.
//!
//! trybuild compiles each `tests/ui/*.rs` as its own crate and compares the
//! diagnostics against the checked-in `.stderr` files. The UI fixtures depend
//! on soroban-sdk, which trybuild compiles into its own target directory on
//! the first run (cached afterwards).

#[test]
fn storage_macro_compile_errors() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
