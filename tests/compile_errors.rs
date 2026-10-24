//! Compile errors and their messages, see `tests/ui/`.
//!
//! The messages in `*.stderr` depend on the rustc version, so the test only
//! runs on Rust 1.84 (the MSRV) and is ignored elsewhere. After an intentional
//! change, update them with
//! `TRYBUILD=overwrite cargo +1.84 test --test compile_errors` and review the diff.

// Before `#[test]`, which only sees an `ignore` that is already there.
#[rustversion::attr(
    not(stable(1.84)),
    ignore = "compiler messages differ between versions; run on Rust 1.84"
)]
#[test]
fn ui() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
