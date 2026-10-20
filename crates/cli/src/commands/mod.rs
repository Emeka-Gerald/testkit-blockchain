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
/// prints it with [`render_error`] and exits 1; it never lets a panic or
/// backtrace reach the user.
///
/// Every command builds its messages to the same conventions, so errors
/// read alike whichever subcommand produced them:
///
/// - Say what failed and, where there is one, what to do about it
///   (`failed to read `x.wasm`: ...; pass the path to a compiled contract`).
/// - Start lowercase and leave off the `error:` prefix; [`render_error`]
///   adds it (and strips a duplicate one).
/// - Wrap user-supplied names, paths and flags with [`quote()`].
/// - Use `Display`, never `{:?}`: no Rust debug output (`Os { code: 2, .. }`,
///   `Map(ScSpecTypeMap { .. })`, escaped strings) reaches the user.
#[derive(Debug)]
pub struct CliError(pub String);

/// Quotes a user-supplied value (a function or parameter name, a path, a
/// flag) inside a [`CliError`] message, as `` `value` ``.
pub fn quote(value: impl std::fmt::Display) -> String {
    format!("`{value}`")
}

/// The text `main` writes to stderr for `err`: `prefix` (normally
/// `error:`), a space, and the message. Surrounding whitespace is trimmed,
/// a leading `error:` the message already carries is dropped so it is
/// never printed twice, and any continuation lines are indented two
/// spaces under the first. An empty message becomes `unknown error`.
pub fn render_error(prefix: &str, err: &CliError) -> String {
    let mut message = err.0.trim();
    while let Some(head) = message.get(..6) {
        if !head.eq_ignore_ascii_case("error:") {
            break;
        }
        message = message[6..].trim_start();
    }
    if message.is_empty() {
        message = "unknown error";
    }

    let mut lines = message.lines();
    let mut rendered = format!("{prefix} {}", lines.next().unwrap_or_default().trim_end());
    for line in lines {
        rendered.push('\n');
        if !line.trim().is_empty() {
            rendered.push_str("  ");
            rendered.push_str(line.trim_end());
        }
    }
    rendered
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn render(message: &str) -> String {
        render_error("error:", &CliError(message.to_string()))
    }

    #[test]
    fn single_line_messages_get_one_prefix() {
        assert_eq!(render("probe failed"), "error: probe failed");
        assert_eq!(render("  probe failed \n"), "error: probe failed");
    }

    #[test]
    fn a_duplicate_error_prefix_is_dropped() {
        assert_eq!(render("error: probe failed"), "error: probe failed");
        assert_eq!(render("Error:error: probe failed"), "error: probe failed");
    }

    #[test]
    fn continuation_lines_are_indented_under_the_first() {
        assert_eq!(
            render("coverage run failed\nhint: install cargo-llvm-cov\n\ndone"),
            "error: coverage run failed\n  hint: install cargo-llvm-cov\n\n  done"
        );
    }

    #[test]
    fn an_empty_message_still_says_something() {
        assert_eq!(render(""), "error: unknown error");
        assert_eq!(render("error:  "), "error: unknown error");
    }

    #[test]
    fn the_colored_prefix_is_used_verbatim() {
        assert_eq!(
            render_error("\x1b[1;31merror:\x1b[0m", &CliError("x".into())),
            "\x1b[1;31merror:\x1b[0m x"
        );
    }

    #[test]
    fn quote_wraps_values_in_backticks() {
        assert_eq!(quote("batch_payout"), "`batch_payout`");
        assert_eq!(
            quote(std::path::Path::new("a/b.wasm").display()),
            "`a/b.wasm`"
        );
    }
}
