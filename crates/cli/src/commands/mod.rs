pub mod audit;
pub mod coverage;
pub mod limits;
pub mod output;

use clap::{Parser, Subcommand};

/// Coverage, resource-limit discovery, and static audit checks for Soroban
/// contracts.
///
/// Every command runs locally and makes no network access: `coverage` runs
/// your tests under `cargo llvm-cov`, `limits` invokes a compiled .wasm in
/// an in-process Soroban host, and `audit` only reads source files.
#[derive(Parser)]
#[command(
    name = "soroban-testkit",
    verbatim_doc_comment,
    version,
    after_help = "EXIT STATUS:\n    \
    0  success\n    \
    1  the command failed; the reason is printed on stderr as `error: ...`\n    \
    2  invalid usage (unknown flag, missing argument, ...)\n\n\
ENVIRONMENT:\n    \
    NO_COLOR  a non-empty value has the same effect as --no-color\n\n\
Run `soroban-testkit <COMMAND> --help` for each command's defaults and limitations."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: output::GlobalArgs,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Measure test coverage of a contract crate via cargo-llvm-cov
    ///
    /// Runs `cargo llvm-cov test` in the current directory. Contract tests
    /// run natively in soroban-sdk's test harness, not against the compiled
    /// .wasm, so ordinary LLVM source-based coverage applies with no
    /// Soroban-specific flags.
    ///
    /// Requires cargo-llvm-cov (`cargo install cargo-llvm-cov`); this
    /// command never installs it. It makes no network access itself, but
    /// cargo may fetch dependencies the crate has not downloaded yet.
    #[command(verbatim_doc_comment)]
    Coverage(coverage::CoverageArgs),
    /// Find the largest input a contract function handles before it fails
    ///
    /// Invokes --fn on the compiled .wasm in a local Soroban host (no
    /// network), increasing the --ramp parameter: doubling from 1 until a
    /// call fails, then binary-searching between the last success and the
    /// first failure. Each attempt runs in its own subprocess. Values above
    /// 16777216 (2^24) are not tried. Prints the last successful value with
    /// the CPU instructions and memory bytes it used.
    ///
    /// Limitations:
    ///   - --ramp must be u32/i32/u64/i64/u128/i128 (ramped as the value
    ///     itself) or `Vec<Address>` (ramped as its length).
    ///   - Other parameters get fixed defaults: a generated Address (one
    ///     named `token` gets a funded Stellar Asset Contract), `false`, 1
    ///     for integers, Symbol `x`, and an empty String, Bytes or Vec. Map,
    ///     Tuple, Option, Result and user-defined types are rejected.
    ///   - All authorization is mocked as approved.
    ///   - Ledger reads/writes and transaction size are not measured; they
    ///     need network-backed transaction simulation.
    #[command(verbatim_doc_comment)]
    Limits(limits::LimitsArgs),
    /// Static checks over a contract crate (not a security product)
    ///
    /// Parses every .rs file under PATH and reports three patterns:
    ///   - a function taking an Address parameter that never calls
    ///     require_auth
    ///   - arithmetic (+ - * /) on an i128-typed binding outside a
    ///     checked_/saturating_/wrapping_ call
    ///   - a persistent()/temporary() storage read with no extend_ttl in the
    ///     same function
    ///
    /// These are syntactic heuristics: findings can be false positives, and
    /// no findings does not mean a contract is secure. Files that fail to
    /// parse are skipped. Findings exit 0 unless --strict is passed.
    #[command(verbatim_doc_comment)]
    Audit(audit::AuditArgs),
    /// Hidden: runs a single limits probe in its own process. See
    /// `limits::run`'s doc comment for why.
    #[command(name = "__limits-probe", hide = true)]
    LimitsProbe(limits::ProbeArgs),
}

/// A command failure with an actionable, user-facing message. `main`
/// prints this and exits non-zero; it never lets a panic or backtrace
/// reach the user.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

impl From<String> for CliError {
    fn from(value: String) -> Self {
        CliError(value)
    }
}

impl From<&str> for CliError {
    fn from(value: &str) -> Self {
        CliError(value.to_string())
    }
}

impl From<std::io::Error> for CliError {
    fn from(value: std::io::Error) -> Self {
        CliError(format!("I/O error: {value}"))
    }
}
