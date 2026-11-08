//! Compile errors and their messages, see `tests/ui/` and `tests/ui-no-alloc/`.
//!
//! The messages in `*.stderr` depend on the rustc version, so the tests only
//! run on Rust 1.84 (the MSRV) and are ignored elsewhere. After an intentional
//! change, update them with
//! `TRYBUILD=overwrite cargo +1.84 test --test compile_errors` (once with and
//! once without `--all-features`) and review the diff.

// Before `#[test]`, which only sees an `ignore` that is already there.
#[rustversion::attr(
    not(stable(1.84)),
    ignore = "compiler messages differ between versions; run on Rust 1.84"
)]
#[test]
fn ui() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}

/// Errors that only occur without the `alloc` feature.
#[cfg(not(feature = "alloc"))]
#[rustversion::attr(
    not(stable(1.84)),
    ignore = "compiler messages differ between versions; run on Rust 1.84"
)]
#[test]
fn ui_without_alloc() {
    trybuild::TestCases::new().compile_fail("tests/ui-no-alloc/*.rs");
}
