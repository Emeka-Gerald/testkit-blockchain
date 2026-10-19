//! Test-only helpers shared by this crate's unit tests.
//!
//! Assertion helpers panic with a [`TestkitError`], but the stable
//! machine-readable code is deliberately kept out of the display message
//! (see [`TestkitError::code`]). Tests that care about *which* failure
//! occurred — rather than the exact wording — use
//! [`assert_panics_with_code`] instead of hand-rolled `catch_unwind` and
//! string matching.
//!
//! This module is compiled only under `cfg(test)`: it is not part of the
//! crate's public API.

use std::panic::{self, AssertUnwindSafe};

use crate::core::TestkitError;
use crate::internal::panic_message;

/// The display prefix [`TestkitError`] uses for the variant carrying
/// `code`, or `None` if `code` is not a documented code.
///
/// Kept next to the panic helpers so a new variant fails the unit test in
/// this module until it is added here too.
fn prefix_for_code(code: &str) -> Option<&'static str> {
    match code {
        "TESTKIT_ASSERTION_FAILED" => Some("assertion failed"),
        "TESTKIT_DECODE_FAILED" => Some("failed to decode value"),
        "TESTKIT_MISUSE" => Some("misuse of testkit API"),
        _ => None,
    }
}

/// Run `f`, require it to panic, and return the panic message.
///
/// Panics — failing the calling test — if `f` returns normally, so a test
/// can assert both "this must fail" and details of the failure in one
/// place.
#[track_caller]
pub(crate) fn catch_panic(f: impl FnOnce()) -> String {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(()) => panic!("expected the closure to panic, but it returned normally"),
        Err(payload) => panic_message(payload.as_ref()),
    }
}

/// Assert that `f` panics with the [`TestkitError`] variant identified by
/// the stable `code`, returning the panic message for further assertions.
///
/// Matching is on the variant's display prefix rather than the whole
/// message, so tests stay robust to rewording while still pinning the
/// error category. Panics if `code` is not a documented code, if `f`
/// returns normally, or if the panic came from a different variant.
///
/// ```ignore
/// assert_panics_with_code("TESTKIT_MISUSE", || env.token_with_decimals(6));
/// ```
#[track_caller]
pub(crate) fn assert_panics_with_code(code: &str, f: impl FnOnce()) -> String {
    let Some(prefix) = prefix_for_code(code) else {
        panic!("unknown TestkitError code {code:?}");
    };
    let message = catch_panic(f);
    assert!(
        message.starts_with(prefix),
        "expected a panic carrying code {code} (display prefix {prefix:?}), got: {message}"
    );
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_errors() -> [TestkitError; 3] {
        [
            TestkitError::AssertionFailed("assertion detail".into()),
            TestkitError::DecodeFailed("decode detail".into()),
            TestkitError::Misuse("misuse detail".into()),
        ]
    }

    #[test]
    fn every_documented_code_maps_to_its_variant_display_prefix() {
        for err in all_errors() {
            let prefix = prefix_for_code(err.code())
                .unwrap_or_else(|| panic!("{} has no display prefix", err.code()));
            assert!(
                err.to_string().starts_with(prefix),
                "{} display {:?} does not start with {prefix:?}",
                err.code(),
                err.to_string()
            );
        }
    }

    #[test]
    fn assert_panics_with_code_accepts_a_matching_variant() {
        let message = assert_panics_with_code("TESTKIT_MISUSE", || {
            panic!("{}", TestkitError::Misuse("bad call".into()));
        });
        assert!(message.contains("bad call"), "{message}");
    }

    #[test]
    #[should_panic(expected = "expected a panic carrying code TESTKIT_MISUSE")]
    fn assert_panics_with_code_rejects_a_different_variant() {
        assert_panics_with_code("TESTKIT_MISUSE", || {
            panic!("{}", TestkitError::AssertionFailed("not a misuse".into()));
        });
    }

    #[test]
    #[should_panic(expected = "returned normally")]
    fn assert_panics_with_code_rejects_a_non_panicking_closure() {
        assert_panics_with_code("TESTKIT_MISUSE", || {});
    }

    #[test]
    #[should_panic(expected = "unknown TestkitError code \"NOT_A_CODE\"")]
    fn assert_panics_with_code_rejects_an_unknown_code() {
        assert_panics_with_code("NOT_A_CODE", || {});
    }
}
