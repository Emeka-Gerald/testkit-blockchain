# soroban-testkit

**The testing crate you would have written yourself, on the third contract.**

Testing infrastructure for Soroban contracts: property-test generators for
money math, an event-assertion API, ledger time and TTL-expiry control,
token test doubles, authorization matrices, and a coverage-aware CLI.

`soroban-sdk` ships `testutils`, which is adequate for basic unit tests. It
is not adequate for testing contracts that hold money. Every serious
Soroban contract repo independently rebuilds some subset of:

- Advancing ledger time correctly, including the interaction between
  `timestamp` and `sequence`
- Asserting on emitted events without hand-decoding `Val` topics
- Generating adversarial `i128` values that actually find overflow bugs
- Deploying and funding a Stellar Asset Contract token double for transfer
  tests
- Proving that every privileged entry point rejects unauthorized callers
- Simulating TTL expiry

`soroban-testkit` is that shared layer, as a dev-dependency crate plus a
small CLI.

## For Soroban developers

**Keywords:** Soroban testing, smart contracts, Stellar, WASM testing, property testing, money math, event assertions, TTL control, test coverage, resource limits, audit tools.

If you're building a Soroban contract that handles tokens, transfers, or funds, or if you need property-test generators for money math, event-assertion APIs, or TTL expiry simulation, this crate is designed for you. Use it as a dev-dependency in your contract crate for unit and integration tests, or use the CLI for coverage analysis and static audits.

## Example

A small escrow contract, tested end to end: mint a token, release it to
the seller, and check the event, the balances, and that no value was
created or destroyed along the way.

```
use soroban_sdk::{contract, contractimpl, symbol_short, Address, Env};
use soroban_testkit::prelude::*;

#[contract]
struct Escrow;

#[contractimpl]
impl Escrow {
    pub fn release(env: Env, token: Address, from: Address, to: Address, amount: i128) {
        from.require_auth();
        soroban_sdk::token::TokenClient::new(&env, &token)
            .transfer(&from, soroban_sdk::MuxedAddress::from(to), &amount);
        #[allow(deprecated)]
        env.events().publish((symbol_short!("release"),), amount);
    }
}

# fn main() {
let env = TestEnv::new();
let token = env.token();
let [buyer, seller]: [soroban_sdk::Address; 2] =
    env.addresses(2).try_into().unwrap();
// `mint` takes whole tokens and scales by 10^decimals (7 for a SAC);
// `mint_raw` takes base units (stroops) as-is. Both lines below fund
// the buyer with 100 tokens = 1_000_000_000 base units.
token.mint(&buyer, 100);
let other_buyer = env.address();
token.mint_raw(&other_buyer, 1_000_000_000);
assert_eq!(token.balance(&buyer), token.balance(&other_buyer));

let escrow_id = env.env().register(Escrow, ());
let client = EscrowClient::new(env.env(), &escrow_id);

env.env().mock_all_auths();
// Capture scope: `events` holds only what this closure's call emitted.
// Any later call, including a balance query, starts a new top-level
// invocation, so inspect `events` here rather than re-reading the log
// after subsequent calls. See EventLog's note on scope.
let (_, events) = env.events_during(|| {
    client.release(&token.address(), &buyer, &seller, &1_000_000_000);
});
events
    .from(&escrow_id)
    .assert_emitted(symbol_short!("release"));

// Balances are asserted (in base units) only after the events above
// were read.
token.assert_balance(&buyer, 0);
token.assert_balance(&seller, 1_000_000_000);

Conservation {
    deposited: 1_000_000_000,
    withdrawn: 1_000_000_000,
    refunded: 0,
    remaining: 0,
}
.assert_holds();
# }
```

### Modeling nested authorization

When a privileged entry point makes an authorized sub-invocation, put this
shape inside the invoke closure passed to `AuthMatrix::entry_point`. The
root invocation is the entry point under test; each nested invocation
belongs in its parent's `MockAuthInvoke::sub_invokes` slice. This lets
`AuthMatrix` check the same authorization shape that the contract executes
instead of granting one flat authorization to the whole call.

```text
let result = Client::new(env, &parent_id)
  .mock_auths(&[MockAuth {
    address: &caller,
    invoke: &MockAuthInvoke {
      contract: &parent_id,
      fn_name: "withdraw",
      args: (child_id.clone(), amount).into_val(env),
      sub_invokes: &[MockAuthInvoke {
        contract: &child_id,
        fn_name: "authorize",
        args: (caller.clone(), amount).into_val(env),
        sub_invokes: &[],
      }],
    },
  }])
  .try_withdraw(&child_id, &amount);
```

Use the generated client's actual argument tuple and `try_*` method for your
contract. The [auth module documentation](crates/testkit/src/auth/mod.rs),
including the [`AuthMatrix` Rustdoc](crates/testkit/src/auth/matrix.rs),
contains the complete matrix contract and failure behavior; the
`MockAuthInvoke` tree above is only the nested-call part.

## CLI

A companion binary, `soroban-testkit-cli` (installs as `soroban-testkit`),
for things that don't belong in a dev-dependency:

### Installation

Install from crates.io:

```sh
cargo install soroban-testkit-cli --locked
```

Verify the installation:

```sh
soroban-testkit --help
```

Install from a local checkout:

```sh
cargo install --path crates/cli --locked
```

Install to a temporary directory for validation:

```sh
TMP_DIR=$(mktemp -d)
CARGO_INSTALL_ROOT="$TMP_DIR" cargo install --path crates/cli --locked
"$TMP_DIR/bin/soroban-testkit" --help
rm -rf "$TMP_DIR"
```

### Usage

```sh
# Coverage for native contract tests (wraps cargo-llvm-cov, which needs no
# Soroban-specific flags since unit/integration tests run natively in the
# test harness, not against the compiled .wasm).
soroban-testkit coverage --format html --open

# Empirically find how many recipients a batch operation can handle
# before it exceeds mainnet resource limits.
soroban-testkit limits --contract target/wasm32v1-none/release/my_contract.wasm \
  --fn batch_payout --ramp recipients

# Static checks: missing require_auth, unchecked i128 arithmetic,
# storage reads with no TTL bump. Not a security product.
soroban-testkit audit ./src --strict
```

### Failure examples

Each command exits non-zero on failure, with the reason on stderr (#249):

```sh
# coverage: cargo-llvm-cov not installed, or the run itself fails
$ soroban-testkit coverage
error: failed to run `cargo llvm-cov` (No such file or directory); is cargo-llvm-cov installed? try `cargo install cargo-llvm-cov`
$ soroban-testkit coverage --fail-under 90
error: coverage run failed (cargo llvm-cov exited with exit status: 101)

# limits: unknown --fn or --ramp parameter name
$ soroban-testkit limits --contract target/wasm32v1-none/release/my_contract.wasm \
  --fn batch_payout --ramp nonexistent_param
error: `batch_payout` has no parameter named `nonexistent_param`; its parameters are `recipients`, `amount`

# limits: a parameter whose type the command can't build a value for
error: parameter `balances` has type `Map<Symbol, u32>`, which this command doesn't know how to default yet; ...

# audit: PATH doesn't exist
$ soroban-testkit audit ./scr
error: cannot audit `./scr`: No such file or directory (os error 2); pass a contract crate's source directory, a .rs file, or `-` for stdin

# audit --strict: exits non-zero when findings exist (printed above this line)
$ soroban-testkit audit ./src --strict
error: 2 audit finding(s) (--strict)
```

Every command reports failures the same way: exit status 1 and a single
`error: <message>` on stderr (clap's own usage errors also start with
`error:`, but exit 2). Messages say what failed and, where there is one,
the fix; user-supplied names, paths and types are wrapped in backticks,
and contract types are shown as Rust-like names (`Vec<Address>`), never
as Rust debug output or a panic backtrace. The conventions live on
`CliError` in `crates/cli/src/commands/mod.rs`.

`soroban-testkit audit` is a linter with three heuristics, not a security
product. It can miss real bugs and flag non-bugs; a missing finding is not a
security guarantee. This is the same bounded claim made by the command output
and the Module 9 requirements in [`BUILD_SPEC.md`](BUILD_SPEC.md#soroban-testkit-audit).
See [Audit command limitations](#audit-command).

`limits` ramps a numeric parameter directly, or generates addresses for a
`Vec<Address>` parameter — the common "how many recipients" question.
Every ramp attempt runs in its own subprocess: a real `.wasm` contract
that exceeds resource limits can abort the process outright rather than
return an error, and isolating each attempt is the only safe way to probe
past that boundary. Ledger read/write counts and transaction size aren't
reported (they come from a network-side simulated footprint this crate
doesn't produce); instructions and memory, measured locally, are. See
[Limits command limitations](#limits-command).

A run prints the last successful ramp value and exactly two measured
fields, both from the local SDK budget:

```text
last successful recipients for "batch_payout": 37
  instructions: 98213456
  memory bytes: 4718592
  (ledger reads/writes and transaction size are not measured by this command; see --help)
```

The numbers above are illustrative; the field names are not. `instructions`
is the CPU-instruction count and `memory bytes` the memory budget consumed by
one call at the reported value. Nothing else is measured.

## Known limitations

These constraints follow from the current `soroban-sdk` test environment and
the project's network-free boundary. They are supported boundaries, not
claims about live-network behavior.

### TTL helpers

- TTL inspection and expiry use `soroban-sdk::testutils` in the current
  in-memory environment. They do not fetch a network's archival configuration
  or exercise restore-footprint operations.
- `ttl_of` follows the SDK behavior and panics when a persistent or temporary
  entry is missing or already expired. Instance storage has one TTL per
  contract, so its `key` argument is ignored.
- `expire(kind)` is intentionally coarse. It reads the environment's current
  `max_entry_ttl` and advances the whole ledger beyond it; `kind` documents the
  caller's intent but does not restrict expiry to one entry or storage kind.
  Use a disposable `TestEnv` when unrelated state must survive.

#### Expiry versus archival

Temporary storage is intentionally disposable: after its TTL expires, the
entry is gone and a contract must handle the missing value. Persistent
storage is different: an expired entry is archival state that can be brought
back by a live-network restore-footprint workflow, then rewritten or
extended. The testkit does not model that workflow.

The supported network-free check is therefore to test the contract's
post-expiry behavior, and to test TTL extension while an entry is still
present:

```text
env.assert_bumps_ttl(&contract_id, StorageKind::Persistent, key, || {
    client.touch_record_checked();
});

env.assert_survives_expiry(StorageKind::Temporary, || {
    client.read_temp_checked(); // handles the missing entry
});
```

Do not treat `expire` as proof that persistent archival can be restored, or
that a temporary entry can be recovered. Those network and restore-footprint
semantics are outside the current SDK testutils boundary.

### Limits command

- `limits` registers the supplied WASM in a fresh local SDK environment for
  every probe, allows mocked authorization, and reports local CPU-instruction
  and memory budgets. It is a diagnostic ceiling, not a network simulation or
  a guarantee that the same call fits a live transaction.
- Ramp parameters are limited to integer types and `Vec<Address>`. Other
  parameters receive fixed defaults only for the supported primitive types;
  a parameter named `token` receives a funded SAC as a documented heuristic.
- Contracts must currently register with empty constructor arguments. Support
  for constructor arguments is tracked in [#242](https://github.com/soroban-testkit/testkit-blockchain/issues/242).
- Ledger read/write counts and transaction size are unavailable because they
  require the network-side simulated footprint that this command deliberately
  does not produce.

### Audit command

`audit` checks source syntax for three local patterns:

- `missing-require-auth`: a function takes an `Address` parameter and its
  body never calls `require_auth`.
- `unchecked-i128-arithmetic`: `+`, `-`, `*`, or `/` on a value typed `i128`
  in the same function, instead of a `checked_`/`saturating_`/`wrapping_`
  method.
- `missing-ttl-bump`: a persistent or temporary storage `get` with no
  `extend_ttl` call in the same function body.

It does not expand macros, follow calls into helper functions, or replace a
contract security review, so both false positives and false negatives are
expected. Every run that isn't `--quiet`, including one with no findings,
ends with the same disclaimer: a missing finding is not a security guarantee.

## Roadmap

The supported surface today is deliberately in-process and network-free:
isolated test environments, ledger-clock control, adversarial money values,
event assertions, SAC token doubles, authorization matrices, TTL-expiry
checks, and the `coverage`, `limits`, and three-rule `audit` CLI commands.
The next increments are kept small and tracked in these curated issues:

| Area | Planned increment | Tracking issues |
|---|---|---|
| Ledger ergonomics | Sequence-oriented advancement and reusable clock checkpoints | [#60](https://github.com/soroban-testkit/testkit-blockchain/issues/60), [#61](https://github.com/soroban-testkit/testkit-blockchain/issues/61) |
| TTL assertions | Minimum-TTL and exact TTL-delta assertions | [#187](https://github.com/soroban-testkit/testkit-blockchain/issues/187), [#188](https://github.com/soroban-testkit/testkit-blockchain/issues/188) |
| Audit workflow | Rule selection, documented suppressions, baselines, and rule explanations | [#215](https://github.com/soroban-testkit/testkit-blockchain/issues/215), [#216](https://github.com/soroban-testkit/testkit-blockchain/issues/216), [#220](https://github.com/soroban-testkit/testkit-blockchain/issues/220), [#221](https://github.com/soroban-testkit/testkit-blockchain/issues/221) |
| Limits CLI | Constructor arguments for contracts that cannot be registered with `()` | [#242](https://github.com/soroban-testkit/testkit-blockchain/issues/242) |
| Real-contract validation | Recurring-contract and batch-payout authorization coverage | [#266](https://github.com/soroban-testkit/testkit-blockchain/issues/266), [#267](https://github.com/soroban-testkit/testkit-blockchain/issues/267) |

Issues describe intended work, not release promises. Mainnet/testnet forking,
deployment tooling, frontend testing, a test runner, a full benchmarking
framework, and network-dependent test behavior remain out of scope; see
[`BUILD_SPEC.md` §3](BUILD_SPEC.md#3-scope-boundaries).

`audit` reports each finding's path relative to the audited `PATH`, with
`/` separators, so `soroban-testkit audit src` run from the crate and
`soroban-testkit audit my_contract/src` run from its parent both print
`lib.rs:12: ...`. When `PATH` is a single file, its file name is used.

### Coverage thresholds

`--fail-under PCT` makes `coverage` exit non-zero when **line** coverage is
below `PCT`. It is forwarded unchanged to cargo-llvm-cov as
`--fail-under-lines PCT`, so cargo-llvm-cov does the measuring and the
comparison; it combines with any `--format`. Requires
`cargo install cargo-llvm-cov`. Running the command locally makes no
network access.

```sh
# Exits 0 if line coverage is >= 80%, otherwise prints the report and
# fails with: error: coverage run failed (cargo llvm-cov exited with ...)
soroban-testkit coverage --fail-under 80

# Same threshold, writing lcov.info for an IDE or CI upload step.
soroban-testkit coverage --format lcov --fail-under 92.5

# What actually runs (shown with --verbose):
#   cargo llvm-cov test --fail-under-lines 80
soroban-testkit --verbose coverage --fail-under 80

# Print that command without running it; needs neither cargo nor
# cargo-llvm-cov installed:
#   cargo llvm-cov test --lcov --output-path lcov.info --fail-under-lines 80
soroban-testkit coverage --format lcov --fail-under 80 --dry-run
```

In CI, run it as an ordinary step; its exit code gates the job.

### Coverage JSON summary

`--format json` writes a machine-readable summary to `coverage.json` in the
current directory, for CI steps that post or compare coverage numbers. It
runs `cargo llvm-cov test --json --summary-only --output-path coverage.json`,
so the file is LLVM's coverage export (`"type": "llvm.coverage.json.export"`,
`"version": "2.x"`) with totals and per-file summaries but no per-line data.

After the run, `coverage` checks the file against the fields below and fails
with `error: coverage.json is not a valid coverage summary: ...` if any are
missing or malformed:

```json
{
  "type": "llvm.coverage.json.export",
  "version": "2.0.1",
  "data": [{
    "totals": {
      "lines":     { "count": 40, "covered": 34, "percent": 85.0 },
      "functions": { "count": 10, "covered": 9,  "percent": 90.0 },
      "regions":   { "count": 60, "covered": 48, "percent": 80.0 }
    },
    "files": [{ "filename": "src/lib.rs", "summary": { "lines": { "count": 40, "covered": 34, "percent": 85.0 } } }]
  }]
}
```

`count` and `covered` are non-negative integers with `covered <= count`, and
`percent` is a number in `0..=100`. LLVM may add other fields (such as
`branches`); consumers should ignore them. On success it prints
`wrote coverage.json: line coverage 85.00% (34/40 lines)`, which `--quiet`
suppresses. It combines with `--fail-under` and makes no network access.

```sh
soroban-testkit coverage --format json --fail-under 80
jq '.data[0].totals.lines.percent' coverage.json
```
`--dry-run` is also how the flag forwarding is tested: the CLI's unit tests
check the full command (program, arguments, and the `CARGO_TERM_COLOR` /
`NO_COLOR` environment it sets) built by `coverage::Invocation`, and its
integration tests run the real binary with `--dry-run` and an empty `PATH`,
so `cargo test` never needs cargo-llvm-cov. `--open` combines only with
`--format html` (or the default format, which it switches to HTML);
`--format lcov --open` is rejected, since cargo-llvm-cov can't write both in
one run.

### Output controls

These global flags go before or after the subcommand:

| Flag | Effect |
| --- | --- |
| `--no-color` | No ANSI color codes, including from the `cargo` that `coverage` runs. A non-empty `NO_COLOR` environment variable does the same. Color is only ever written to a terminal, so piped output is already plain. |
| `-q`, `--quiet` | Results and errors only: drops advisory notes such as the `audit` disclaimer and "no findings". Passes `--quiet` to cargo-llvm-cov. |
| `-v`, `--verbose` | Extra diagnostics on stderr (files scanned, commands run, each `limits` probe). Passes `--verbose` to cargo-llvm-cov. Stdout is unchanged. |

`--quiet` and `--verbose` cannot be combined. Findings, measured limits,
`--json` documents, and errors are printed at every level.

## Offline install

Contributors working without network access to crates.io can prefetch and
verify dependencies ahead of time, then build and test fully offline:

```sh
# On a machine with network access: fetch every dependency into the
# local Cargo registry cache and vendor it alongside the workspace.
cargo vendor --locked vendor > cargo-vendor-config.toml

# Copy `vendor/` and `cargo-vendor-config.toml` to the offline machine,
# then point Cargo at the vendored sources instead of crates.io by
# appending the generated config to .cargo/config.toml:
mkdir -p .cargo
cat cargo-vendor-config.toml >> .cargo/config.toml
```

With the vendor directory in place, `cargo build`, `cargo test
--workspace`, and `cargo doc --workspace` all resolve dependencies from
`vendor/` with no network access. `cargo vendor --locked` fails loudly if
`Cargo.lock` and the registry disagree, so re-run it after any dependency
bump rather than editing `vendor/` by hand. Verify the result before
relying on it:

```sh
cargo test --workspace --offline
```

`--offline` makes Cargo error immediately if any dependency would need a
network fetch, instead of silently falling back to it.

## Supported versions

- **Rust:** the `stable` channel pinned in
  [`rust-toolchain.toml`](rust-toolchain.toml). There is no separate MSRV —
  the pinned stable toolchain is the only version CI runs against. See
  [CONTRIBUTING.md's Rust edition policy](CONTRIBUTING.md#rust-edition-policy)
  for how and when that pin moves.
- **`soroban-sdk`:** the version pinned in `[workspace.dependencies]` in the
  root `Cargo.toml` is the only version this crate is tested against for
  every PR. A weekly `scheduled-sdk-check` CI job additionally builds and
  tests the workspace against whatever version crates.io currently
  considers latest, independent of that pin, and opens a tracking issue if
  a newer release breaks compatibility — so drift is caught before a
  contributor hits it, without requiring every PR to track upstream.

## Status

This crate is under active development. See `BUILD_SPEC.md` for the build
plan and module boundaries, and [`ARCHITECTURE.md`](ARCHITECTURE.md) for a
contributor-facing map of the workspace and how its modules fit together.

## Prior art

[`soroban-fork`](https://crates.io/crates/soroban-fork) does lazy
mainnet/testnet forking for tests. That is a different problem;
`soroban-testkit` does not attempt it.

## License

Apache-2.0
