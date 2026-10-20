//! One-line Stellar Asset Contract test doubles.
//!
//! # Example
//!
//! ```
//! use soroban_testkit::core::TestEnv;
//!
//! // Setup: a fresh environment, a standard token and a recipient.
//! let env = TestEnv::new();
//! let token = env.token();
//! let alice = env.address();
//!
//! // Operation: mint 500 units to `alice`.
//! token.mint(&alice, 500);
//!
//! // Assertion: her balance reflects the mint (500 whole tokens at 7
//! // decimals = 5_000_000_000 base units).
//! token.assert_balance(&alice, 5_000_000_000);
//! ```

mod sac;

pub use sac::TestToken;
