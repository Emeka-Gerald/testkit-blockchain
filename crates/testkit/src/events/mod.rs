//! Assert on contract events without hand-decoding `Val` topics.
//!
//! # Example
//!
//! ```
//! use soroban_sdk::{contract, contractimpl, symbol_short, Env, Symbol};
//! use soroban_testkit::core::TestEnv;
//!
//! #[contract]
//! struct Emitter;
//!
//! #[contractimpl]
//! impl Emitter {
//!     pub fn emit(env: Env, topic: Symbol, value: i128) {
//!         #[allow(deprecated)]
//!         env.events().publish((topic,), value);
//!     }
//! }
//!
//! # fn main() {
//! // Setup: register the contract and get a client for it.
//! let env = TestEnv::new();
//! let id = env.env().register(Emitter, ());
//! let client = EmitterClient::new(env.env(), &id);
//!
//! // Operation: invoke the contract, which publishes one event.
//! client.emit(&symbol_short!("transfer"), &100);
//!
//! // Assertion: the event is found by topic, with no manual `Val` decoding.
//! env.events().assert_emitted(symbol_short!("transfer"));
//! # }
//! ```

mod assertions;
mod capture;

pub use capture::{CapturedEvent, EventLog};
