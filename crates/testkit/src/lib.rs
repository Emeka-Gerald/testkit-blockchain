#![doc = include_str!("../../../README.md")]
#![deny(missing_docs)]

// This crate root intentionally re-exports nothing. Primary types
// (`TestEnv`, `TestkitError`, `EventLog`, `Conservation`, `AuthMatrix`,
// `TestToken`, `StorageKind`, …) are not available as `soroban_testkit::Foo`.
// Reach them either through `soroban_testkit::prelude::*` (the supported
// everyday path) or through their owning module, e.g.
// `soroban_testkit::core::TestEnv`, for callers who want explicit paths
// instead of a glob import. See `prelude` for the full re-export list.

// ## Compatibility (public helpers and errors)
//
// Public helpers exported by [`prelude`] and their owning modules are part
// of this crate's public surface. Within a minor version only additive
// changes are made: new methods and new modules may be added, but existing
// signatures are not changed or removed.
//
// [`TestkitError::code()`] is the stable, machine-readable identifier for a
// failure. The human-readable message remains intended for people and may
// be reworded; codes are part of the compatibility contract, never change,
// and are never reused for a different variant. New variants get new codes.
//
// The library has zero non-dev network dependencies and compiles for the
// WASM target where applicable; that network-free behavior is preserved in
// every release.

pub mod auth;
pub mod core;
pub mod events;
pub mod ledger;
pub mod money;
pub mod prelude;
pub mod tokens;
pub mod ttl;

mod internal;

#[cfg(test)]
mod testing;

#[cfg(test)]
mod api_diff_check {
    use crate::auth::AuthMatrix;
    use crate::core::{TestEnv, TestkitError};
    use crate::events::{CapturedEvent, EventLog};
    use crate::money::{adversarial_amounts, amounts_in, bps_values, Conservation};
    use crate::tokens::TestToken;
    use crate::ttl::StorageKind;

    #[test]
    fn documented_public_items_exist() {
        // Compile-time presence check for the public API surface documented
        // in the compatibility section. Renaming or removing any of these
        // items fails this test's compilation, which is the intended signal.
        let _env = TestEnv::new();
        let _code = TestkitError::Misuse("x".into()).code();
        let _event: Option<CapturedEvent> = None;
        let _log: Option<EventLog> = None;
        let _amounts = adversarial_amounts();
        let _ = amounts_in(&_env, 0, 0, 0);
        let _bps = bps_values();
        let _conservation = Conservation {
            deposited: 0,
            withdrawn: 0,
            refunded: 0,
            remaining: 0,
        };
        let _matrix: Option<AuthMatrix> = None;
        let _token: Option<TestToken> = None;
        let _kind: Option<StorageKind> = None;
        drop((
            _env,
            _code,
            _event,
            _log,
            _amounts,
            _bps,
            _conservation,
            _matrix,
            _token,
            _kind,
        ));
    }
}
