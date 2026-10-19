use std::process::Command;

use clap::{Args, ValueEnum};

use super::output::{Output, Verbosity};
use super::CliError;

/// Output format for `soroban-testkit coverage`.
#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    /// Per-file coverage stats printed to the terminal.
    Text,
    /// Write ./lcov.info, for IDE and CI integrations (e.g. Coverage Gutters).
    Lcov,
    /// Write a browsable report to target/llvm-cov/html/.
    Html,
}

/// Arguments for `soroban-testkit coverage`.
#[derive(Args)]
#[command(after_help = "EXAMPLES:\n    \
    soroban-testkit coverage --format html --open\n    \
    soroban-testkit coverage --format lcov\n    \
    soroban-testkit coverage --fail-under 80")]
pub struct CoverageArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Exit 1 if line coverage is below PCT percent (0-100). Forwarded to
    /// cargo-llvm-cov as --fail-under-lines. Default: no threshold
    #[arg(long, value_name = "PCT", verbatim_doc_comment)]
    fail_under: Option<f64>,
    /// Open the HTML report in a browser after generating it (implies
    /// --format html)
    #[arg(verbatim_doc_comment)]
    #[arg(long)]
    open: bool,
}

/// Wraps `cargo llvm-cov test`, which handles Soroban's coverage needs
/// correctly out of the box — soroban_sdk's own test harness runs contract
/// logic natively (not through the WASM VM), so ordinary LLVM
/// source-based coverage instrumentation applies without any
/// Soroban-specific flags. This command's job is knowing *that*, and
/// knowing the right output-format flags, not reimplementing coverage
/// collection.
///
/// Requires `cargo-llvm-cov` to be installed
/// (`cargo install cargo-llvm-cov`).
pub fn run(args: CoverageArgs, out: Output) -> Result<(), CliError> {
    let cargo_args = cargo_args(&args, out);
    out.detail(format!("running: cargo {}", cargo_args.join(" ")));

    let mut cmd = Command::new("cargo");
    cmd.args(&cargo_args);
    if !out.color {
        cmd.env("CARGO_TERM_COLOR", "never").env("NO_COLOR", "1");
    }

    let status = cmd.status().map_err(|err| {
        CliError(format!(
            "failed to run `cargo llvm-cov` ({err}); is cargo-llvm-cov installed? \
             try `cargo install cargo-llvm-cov`"
        ))
    })?;

    if !status.success() {
        return Err(CliError(format!(
            "coverage run failed (cargo llvm-cov exited with {status})"
        )));
    }

    Ok(())
}

/// The arguments passed to `cargo` for a coverage run. `--fail-under PCT`
/// becomes cargo-llvm-cov's `--fail-under-lines PCT`, so the threshold is
/// enforced by cargo-llvm-cov itself and a shortfall surfaces as this
/// command's non-zero exit.
fn cargo_args(args: &CoverageArgs, out: Output) -> Vec<String> {
    let mut cargo_args = vec!["llvm-cov".to_string(), "test".to_string()];
    let mut push = |arg: &str| cargo_args.push(arg.to_string());

    match args.format {
        Format::Text => {}
        Format::Lcov => {
            push("--lcov");
            push("--output-path");
            push("lcov.info");
        }
        Format::Html => push("--html"),
    }

    if args.open {
        if !matches!(args.format, Format::Html) {
            push("--html");
        }
        push("--open");
    }

    if let Some(pct) = args.fail_under {
        push("--fail-under-lines");
        push(&pct.to_string());
    }

    if !out.color {
        push("--color");
        push("never");
    }
    match out.verbosity {
        Verbosity::Quiet => push("--quiet"),
        Verbosity::Normal => {}
        Verbosity::Verbose => push("--verbose"),
    }

    cargo_args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(format: Format, fail_under: Option<f64>, open: bool) -> CoverageArgs {
        CoverageArgs {
            format,
            fail_under,
            open,
        }
    }

    fn out(verbosity: Verbosity, color: bool) -> Output {
        Output { verbosity, color }
    }

    #[test]
    fn fail_under_is_forwarded_as_fail_under_lines() {
        let cargo = cargo_args(
            &args(Format::Text, Some(80.0), false),
            out(Verbosity::Normal, true),
        );
        assert_eq!(cargo, ["llvm-cov", "test", "--fail-under-lines", "80"]);

        let cargo = cargo_args(
            &args(Format::Lcov, Some(92.5), false),
            out(Verbosity::Normal, true),
        );
        assert!(
            cargo.ends_with(&["--fail-under-lines".to_string(), "92.5".to_string()]),
            "{cargo:?}"
        );
    }

    #[test]
    fn no_threshold_means_no_fail_under_flag() {
        let cargo = cargo_args(
            &args(Format::Html, None, true),
            out(Verbosity::Normal, true),
        );
        assert_eq!(cargo, ["llvm-cov", "test", "--html", "--open"]);
    }

    #[test]
    fn color_and_verbosity_are_forwarded_to_cargo_llvm_cov() {
        let quiet = cargo_args(
            &args(Format::Text, None, false),
            out(Verbosity::Quiet, false),
        );
        assert_eq!(quiet, ["llvm-cov", "test", "--color", "never", "--quiet"]);

        let verbose = cargo_args(
            &args(Format::Text, None, false),
            out(Verbosity::Verbose, true),
        );
        assert_eq!(verbose, ["llvm-cov", "test", "--verbose"]);
    }
}
