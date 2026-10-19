# Contributing

See [`ARCHITECTURE.md`](ARCHITECTURE.md) for a map of the workspace — crate
and module boundaries, how they depend on each other, and where a new
capability should live — before your first PR.

## Scope

Read `BUILD_SPEC.md` §3 (Scope boundaries) before opening an issue or PR.

### Accepted proposal categories (In Scope)

Anything that helps test a Soroban contract in-process with `soroban-sdk`'s test environment without requiring network connectivity. Examples grounded in `BUILD_SPEC.md` include:

- **Isolated test harness & environment helpers:** Deterministic `soroban_sdk::Env` orchestration, seed helpers, and test isolation (`core`).
- **Ledger time and clock control:** Advancing ledger sequence and timestamp in lockstep, checkpointing, and time travel for time-locked contracts (`ledger`).
- **Money math and invariants:** Generators for edge-case numeric inputs (`i128` overflow, zero amounts, bounded ranges) and balance conservation assertions (`money`).
- **Event assertions and capture:** Typed topic filtering and data extraction from contract events without manual `Val` decoding (`events`).
- **Token test doubles:** In-memory Stellar Asset Contract (SAC) doubles for minting, transferring, and balance verification (`tokens`).
- **Authorization matrices:** Proving authorized calls succeed and unauthorized or mismatched callers are rejected across privilege matrices (`auth`).
- **TTL and storage expiry:** Simulating entry expiration, TTL thresholds, and persistent/temporary/instance rent boundaries in-memory (`ttl`).
- **Contract analysis tooling:** Local WASM code coverage reporting (via `cargo-llvm-cov`), static contract linting heuristics, and resource limit discovery via subprocess execution (`cli`).

### Rejected proposal categories (Out of Scope)

Proposals falling into these categories will be declined with an explanation pointing to dedicated upstream tooling:

- **Mainnet/testnet chain forking:** Cloning remote ledger state or replaying live network state (point contributors to [`soroban-fork`](https://crates.io/crates/soroban-fork)).
- **Deployment & lifecycle operations:** Deployment scripts, migration utilities, contract hash upgrades, or network transaction submission (use [`stellar-cli`](https://github.com/stellar/stellar-cli)).
- **Frontend & cross-language testing:** JavaScript/TypeScript client bindings, UI integration tests, or web wallet adapters (use [`@stellar/stellar-sdk`](https://github.com/stellar/js-stellar-sdk)).
- **Custom test runner:** Replacing `cargo test` or `cargo nextest` with a bespoke runner or custom test harness (use native Cargo tooling).
- **Full benchmarking frameworks:** Criterion-style statistical microbenchmarking suites (the CLI empirically checks execution budget ceilings, not CPU timing benchmarks).
- **Network-dependent test utilities:** Any feature requiring live RPC calls, Horizon queries, or internet access at test time. The library and CLI must remain 100% network-free with zero non-dev network dependencies.

## Workflow

- Trunk-based development: short-lived branches off `main`, squash merge,
  linear history.
- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/),
  enforced in CI.
- Every PR must pass: `cargo fmt --all --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`, `cargo doc` with
  no warnings, `cargo audit`, `cargo deny check` (see
  [Supply-chain policy](#supply-chain-policy)).
  Red CI blocks merge with no maintainer exception.
- CI also runs a scheduled job weekly against whatever `soroban-sdk` version
  is currently latest on crates.io (independent of the pinned version in
  `Cargo.toml`), so a breaking upstream release is caught before it shows up
  in a contributor's PR. It opens an issue automatically if it fails; it
  never blocks a PR.
- Docs are also built the way docs.rs builds them (`make docsrs`), from the
  `[package.metadata.docs.rs]` table in `crates/testkit/Cargo.toml`: one host
  target and `--cfg docsrs`. Change both together.

## CI workflow permissions

Every workflow under `.github/workflows/` declares a top-level
`permissions:` block, and each job gets the least `GITHUB_TOKEN` scope it
needs:

| Workflow / job | Token scope | Why |
|---|---|---|
| `ci.yml` (default for all jobs) | `contents: read` | Check out and build the repo; nothing is written back |
| `ci.yml` → `scheduled-sdk-check` | `contents: read`, `issues: write` | Opens a tracking issue when the weekly latest-SDK run fails; never runs on `pull_request` |
| `ci.yml` → `scheduled-nightly-check` | `contents: read`, `issues: write` | Opens a tracking issue when the weekly nightly-toolchain run fails; never runs on `pull_request` |
| `issue-triage.yml` (default) | none | — |
| `issue-triage.yml` → `label` | `issues: write` | Adds labels from issue-form selections; no checkout |
| `label-sync.yml` (default) | none | — |
| `label-sync.yml` → `sync` | `issues: write` | Reconciles the documented module, docs, and sizing labels on manual dispatch |

Rules for new or changed workflows:

- Keep the top-level block, and never use `write-all` or `read-all`.
- A job that needs a write scope declares its own `permissions:` block
  (restating `contents: read` if it checks out code), with a comment
  explaining why, and is added to the table above.
- Write scopes are never granted to a job that runs on `pull_request`.
- Third-party secrets (e.g. `CODECOV_TOKEN`) are passed only to the step
  that uses them.

The `lint` job and `crates/cli/tests/repo_config.rs` enforce the top-level
block, the absence of `write-all`/`read-all`, and the per-job scopes above.

## Issue triage

New issues opened from the forms get `module/*` (or `docs`) and `size/*`
labels automatically from their "Module" and "Size" dropdowns
(`.github/workflows/issue-triage.yml`, taxonomy in `BUILD_SPEC.md` §13).
When adding a dropdown option, add the matching entry to the workflow's
`LABELS` map in the same PR; `repo_config.rs` fails otherwise.
Maintainers can run `.github/workflows/label-sync.yml` manually to create or
update the documented module, docs, and sizing labels reproducibly.

### Maintainer triage guide (#251)

When an author's own `module/*`/`size/*` dropdown picks don't match the
actual change, or an issue turns out to be out of scope:

1. **Re-label, don't relabel-and-argue.** Swap the `module/*` or `size/*`
   label to match reality (an issue scoped `size/s` that touches three
   crates is `size/m` at least) — a quick comment naming the change is
   enough; no debate needed for a mechanical correction.
2. **Out of scope:** comment with the specific reason (not a one-word
   "wontfix") — e.g. outside `BUILD_SPEC.md`'s modules, duplicates an
   existing issue (link it), or asks for a breaking change without an
   `## API deprecation policy` plan — then close it. Don't leave it open
   unlabeled "to think about later."
3. **Needs more info:** label `docs` only if it's actually a docs gap;
   otherwise ask the specific question in a comment and leave it open.

## Supply-chain policy

Dependencies are checked with [`cargo-deny`](https://embarkstudios.github.io/cargo-deny/),
configured in `deny.toml`, and enforced in CI (`deny` job). It checks:

- **Licenses** — every dependency's license must be in the `allow` list
  (currently the OSI/FSF-approved licenses this project's Apache-2.0
  license is compatible with). A new dependency under a license outside
  that list needs a documented exception in `deny.toml`, not a widened
  `allow` list.
- **Advisories** — no known-yanked crate versions; RustSec advisories are
  checked in the `audit` job (`cargo audit`) and mirrored here.
- **Sources** — dependencies must come from crates.io; an unlisted registry
  or git dependency fails the check unless explicitly allow-listed.
- **Bans** — no wildcard (`*`) version requirements.

Run it locally before adding a dependency:

```sh
cargo install cargo-deny --locked
cargo deny check
```

The CI job also runs `cargo deny check licenses` explicitly. A successful
license check prints no violations and exits with status 0; a dependency whose
license is absent from `deny.toml`'s `allow` list fails that step before the
full policy check runs.

## Rust edition policy

- **Where it is set:** the edition is declared once, in `[workspace.package]`
  in the root `Cargo.toml`. Every workspace member sets
  `edition.workspace = true` and must not hard-code its own edition.
  The `lint` job in CI fails if a member does not inherit it.
- **When it changes:** only after a new Rust edition is released and the
  pinned toolchain (`rust-toolchain.toml`, `stable`) supports it. Edition
  bumps are never folded into a feature or fix PR.
- **How it changes:** one dedicated PR, titled
  `chore(workspace): adopt Rust <year> edition`, that:
  1. updates `edition` under `[workspace.package]`;
  2. runs `cargo fix --edition --workspace` and commits any resulting
     changes in the same PR;
  3. passes the full `make check` and `cargo doc` gates with no
     new warnings.
- **No behavior changes in the same PR.** If an edition migration needs
  a source change that alters behavior, split it out into its own PR first.

## Code conventions

- No `unwrap()` or `expect()` in library code outside tests.
- Assertion helpers panic deliberately (that is their contract), but always
  with `TestkitError` context, never a bare `panic!("...")` message.
- Diagnostics use American English, lowercase sentence starts, and no
  trailing periods. Use backticks for API identifiers and punctuation only
  when it separates readable clauses or structured values. Keep wording
  specific about the expectation and the observed value; treat changes to
  these messages as public developer-experience changes and pin
  representative output in focused tests.
- Every public item needs a doc comment with a runnable `# Example` block.
  `#![deny(missing_docs)]` is enforced from Module 8 onward.
- Every assertion helper needs two tests: one where it passes, one where it
  correctly fails (`#[should_panic(expected = "...")]`, pinning the
  message).
- Failure messages are a first-class feature of this crate. An assertion
  helper whose failure message doesn't tell the user what went wrong and
  what to look at is not done.

## Maintainer SDK review checklist

The scheduled CI job (`scheduled-sdk-check`) builds against whatever
`soroban-sdk` version is currently latest on crates.io and opens an issue
automatically if that build or test run fails. When that issue lands,
work through it in this order:

1. **Read the failure, not just the summary.** Open the scheduled job's
   log and find the first failing step (build error vs. test failure vs.
   doc-build warning) — later failures are often downstream of the first.
2. **Diff the SDK changelog.** Compare the pinned `soroban-sdk` version in
   `Cargo.toml` against the latest, and read every entry in between for
   breaking changes (removed items, changed signatures, new required
   trait methods).
3. **Reproduce locally** against the new version before changing
   anything:
   ```sh
   cargo update -p soroban-sdk --precise <new-version>
   cargo test --workspace
   ```
4. **Fix forward, don't just re-pin.** If the failure is a real breaking
   change, update the affected call sites and tests in the same PR as the
   version bump — a version bump with disabled tests or `#[ignore]` is
   not an acceptable fix.
5. **Update the pinned version safely.** Bump `soroban-sdk` in
   `Cargo.toml` to the version you validated, run `cargo update -p
   soroban-sdk --precise <version>` to sync `Cargo.lock`, and run the full
   `make check` before opening the PR.
6. **Close the scheduled-job issue** by referencing it (`Closes #N`) in
   the version-bump PR, not by closing it manually — so the fix and the
   failure stay linked in history.

### Behavior areas to re-verify

A green build is not enough after a bump. Confirm that the tests still
exercise each surface the testkit wraps from `soroban-sdk`:

- **testutils:** `TestEnv` construction and contract registration
  (`crates/testkit/src/core/env.rs`).
- **ledger:** sequence and timestamp advancement
  (`crates/testkit/src/ledger/clock.rs`).
- **event:** captured event shape and topic decoding
  (`crates/testkit/src/events/capture.rs`).
- **auth:** caller mocking and rejection in `AuthMatrix`
  (`crates/testkit/src/auth/matrix.rs`).
- **TTL:** TTL reads, extension, and expiry simulation
  (`crates/testkit/src/ttl/expiry.rs`).

If the new SDK version is not yet safe to adopt (e.g. an upstream
regression), leave the pin as-is, comment on the issue with what's
blocking the bump, and leave the issue open as a tracking item.

## API deprecation policy

Public API removals and signature changes follow a two-step deprecation
window rather than landing as a breaking change in the same release:

1. **Deprecate first.** Mark the item `#[deprecated(since = "x.y.z", note
   = "...")]`, where the note names the replacement API and, if
   applicable, a removal-target version. Keep the deprecated item fully
   functional — a deprecation warning, not a behavior change.
2. **Document the replacement.** The deprecation note and the changelog
   entry must both say what to migrate to, not just what is going away.
3. **Wait at least one minor release** before removing a deprecated
   public item. Patch releases never remove a deprecated item.
4. **Remove in a dedicated PR**, called out in the changelog under a
   "Breaking changes" heading, once the deprecation window has elapsed.

### Example

```rust
#[deprecated(since = "0.4.0", note = "use `TestEnv::address` instead")]
pub fn new_address(env: &Env) -> Address {
    // ...
}
```

The crate-level `#![deny(missing_docs)]` lint (see
[Code conventions](#code-conventions)) still applies to deprecated items;
a `#[deprecated]` item keeps its doc comment and `# Example` block until
it is removed.

## Package and binary names

The CLI package is `soroban-testkit-cli`, and its `[[bin]]` in
`crates/cli/Cargo.toml` installs a binary named `soroban-testkit`. If you
rename either, update all of these in the same PR: the install and
`--help` examples in `README.md`, `scripts/smoke_test_cli_install.sh`, and
the CLI name checks in `crates/cli/tests/repo_config.rs`.

## Local setup

```sh
rustup show               # installs the pinned toolchain from rust-toolchain.toml
make check                # fmt + clippy + test, same as CI
```

`make fmt` (and CI) run `cargo fmt --all --check`, so formatting is enforced
across every workspace member and example — including test files — not just
the package you happened to build.

## Good first issues

Labeled `good-first-issue`. Module 7 (`ttl`) is not beginner-friendly and
is never labeled as such.
