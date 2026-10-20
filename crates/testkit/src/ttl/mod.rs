//! Ledger-entry TTL inspection and expiry simulation.
//!
//! Contracts that fail to bump TTL break in production through silent data
//! loss, and the failure mode is close to untestable by hand — this module
//! makes it a normal assertion.
//!
//! # Example
//!
//! ```
//! use soroban_sdk::{contract, contractimpl, symbol_short, Env, Symbol};
//! use soroban_testkit::core::TestEnv;
//! use soroban_testkit::ttl::StorageKind;
//!
//! #[contract]
//! struct Store;
//!
//! #[contractimpl]
//! impl Store {
//!     pub fn set(env: Env, key: Symbol, value: i128) {
//!         env.storage().persistent().set(&key, &value);
//!     }
//!     pub fn touch(env: Env, key: Symbol) {
//!         env.storage().persistent().extend_ttl(&key, 5_000, 10_000);
//!     }
//! }
//!
//! # fn main() {
//! // Setup: store a persistent entry.
//! let env = TestEnv::new();
//! let id = env.env().register(Store, ());
//! let client = StoreClient::new(env.env(), &id);
//! client.set(&symbol_short!("k"), &1);
//!
//! // Operation and assertion: `touch` must extend the entry's TTL.
//! env.assert_bumps_ttl(&id, StorageKind::Persistent, symbol_short!("k"), || {
//!     client.touch(&symbol_short!("k"));
//! });
//! # }
//! ```

mod expiry;

pub use expiry::StorageKind;
