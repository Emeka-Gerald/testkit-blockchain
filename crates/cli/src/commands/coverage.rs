use std::process::Command;

use clap::{Args, ValueEnum};

use super::output::{Output, Verbosity};
use super::CliError;

/// Output format for `soroban-testkit coverage`.
#[derive(Clone, Copy, ValueEnum)]
pub enum Format {
    /// Per-file coverage stats printed to the terminal.
    Text,
    /// An `lcov.info` file, for IDE integrations (e.g. Coverage Gutters).
    Lcov,
    /// A browsable HTML report.
    Html,
}

/// Arguments for `soroban-testkit coverage`.
#[derive(Args)]
#[command(after_help = "EXAMPLES:\n    \
    soroban-testkit coverage --format html --open\n    \
    soroban-testkit coverage --format lcov\n    \
    soroban-testkit coverage --fail-under 80\n    \
    soroban-testkit coverage --format lcov --fail-under 80 --dry-run")]
pub struct CoverageArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Fail (non-zero exit) if line coverage is below this percentage.
    #[arg(long, value_name = "PCT")]
    fail_under: Option<f64>,
    /// Open the HTML report after generating it (implies --format html).
    #[arg(long)]
    open: bool,
    /// Print the cargo command this would run, then exit without running
    /// it. Needs neither cargo nor cargo-llvm-cov.
    #[arg(long)]
    dry_run: bool,
    /// Workspace packages to cover; repeat to select multiple packages.
    #[arg(short = 'p', long = "package", value_name = "SPEC")]
    packages: Vec<String>,
    /// Integration test targets to cover; repeat to select multiple targets.
    #[arg(long = "test", value_name = "NAME")]
    tests: Vec<String>,
    /// Arguments forwarded to the test binary after `--`.
    #[arg(last = true, num_args = 0.., allow_hyphen_values = true)]
    test_args: Vec<String>,
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
/// (`cargo install cargo-llvm-cov`), except with `--dry-run`, which only
/// prints the [`Invocation`] (so it can be tested, and inspected, without
/// the tool).
pub fn run(args: CoverageArgs, out: Output) -> Result<(), CliError> {
    let invocation = Invocation::new(&args, out)?;

    if args.dry_run {
        println!("{invocation}");
        return Ok(());
    }

    check_llvm_cov_version()?;
    if args.open {
        ensure_opener_available()?;
    }
    out.detail(format!("running: {invocation}"));
    let status = invocation.command().status().map_err(|err| {
        CliError(format!(
            "failed to run `cargo llvm-cov` ({err}); is cargo-llvm-cov installed? \
             try `cargo install cargo-llvm-cov`"
        ))
    })?;

    if !status.success() {
        if args.open {
            return Err(CliError(format!(
                "coverage run or HTML browser launch failed (cargo llvm-cov exited with {status}); try without --open, then open target/llvm-cov/html/index.html manually; check your browser configuration"
            )));
        }
        return Err(CliError(format!(
            "coverage run failed (cargo llvm-cov exited with {status})"
        )));
    }

    Ok(())
}

/// Everything `coverage` hands to the OS for a run: the program, its
/// arguments, and the environment variables it sets. Built by
/// [`Invocation::new`] without touching the filesystem or spawning
/// anything, so the forwarded flags are unit-testable and `--dry-run` can
/// print exactly what [`run`] would execute.
#[derive(Debug, PartialEq)]
pub struct Invocation {
    /// Always `cargo`; cargo-llvm-cov is a cargo subcommand.
    pub program: &'static str,
    /// Arguments after `cargo`, starting with `llvm-cov test`.
    pub args: Vec<String>,
    /// Extra environment variables for the child process.
    pub envs: Vec<(&'static str, &'static str)>,
}

impl Invocation {
    /// Maps `coverage`'s flags onto cargo-llvm-cov's:
    ///
    /// - `--format lcov` → `--lcov --output-path lcov.info`;
    ///   `--format html` → `--html`; `--format text` adds nothing.
    /// - `--open` → `--open`, plus `--html` unless the format already is
    ///   html. `--open` with `--format lcov` is an error: cargo-llvm-cov
    ///   cannot write lcov and HTML in one run.
    /// - `--fail-under PCT` → `--fail-under-lines PCT`, so the threshold is
    ///   enforced by cargo-llvm-cov itself and a shortfall surfaces as this
    ///   command's non-zero exit.
    /// - `--no-color` → `--color never`, plus `CARGO_TERM_COLOR=never` and
    ///   `NO_COLOR=1` for the tools cargo runs.
    /// - `--quiet`/`--verbose` → `--quiet`/`--verbose`.
    pub fn new(args: &CoverageArgs, out: Output) -> Result<Self, CliError> {
        let mut cargo_args = vec!["llvm-cov".to_string(), "test".to_string()];
        let mut push = |arg: &str| cargo_args.push(arg.to_string());

        match args.format {
            Format::Text => {}
            Format::Lcov if args.open => {
                return Err(CliError(
                    "--open only applies to the HTML report; drop --open, or use \
                     --format html instead of --format lcov"
                        .to_string(),
                ))
            }
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

        for package in &args.packages {
            push("--package");
            push(package);
        }
        for test in &args.tests {
            push("--test");
            push(test);
        }

        if let Some(pct) = args.fail_under {
            push("--fail-under-lines");
            push(&pct.to_string());
        }

        let mut envs = Vec::new();
        if !out.color {
            push("--color");
            push("never");
            envs.push(("CARGO_TERM_COLOR", "never"));
            envs.push(("NO_COLOR", "1"));
        }
        match out.verbosity {
            Verbosity::Quiet => push("--quiet"),
            Verbosity::Normal => {}
            Verbosity::Verbose => push("--verbose"),
        }

        if !args.test_args.is_empty() {
            push("--");
            for arg in &args.test_args {
                push(arg);
            }
        }

        Ok(Self {
            program: "cargo",
            args: cargo_args,
            envs,
        })
    }

    /// The [`Command`] [`run`] spawns.
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(self.program);
        cmd.args(&self.args).envs(self.envs.iter().copied());
        cmd
    }
}

/// Shell-style: `NAME=value ... cargo llvm-cov test ...`. Arguments are
/// printed as-is; none that [`Invocation::new`] builds contain spaces.
impl std::fmt::Display for Invocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, value) in &self.envs {
            write!(f, "{name}={value} ")?;
        }
        write!(f, "{} {}", self.program, self.args.join(" "))
    }
}


const MINIMUM_LLVM_COV_VERSION: (u64, u64, u64) = (0, 6, 0);

fn parse_version(output: &str) -> Option<(u64, u64, u64)> {
    let version = output
        .split_whitespace()
        .find(|part| part.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let numbers: Vec<_> = version
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .next()?
        .split('.')
        .collect();
    if numbers.len() < 3 {
        return None;
    }
    Some((
        numbers[0].parse().ok()?,
        numbers[1].parse().ok()?,
        numbers[2].parse().ok()?,
    ))
}

fn validate_version(output: &str) -> Result<(), CliError> {
    let found = parse_version(output).ok_or_else(|| {
        CliError(format!(
            "cannot parse cargo-llvm-cov version from {output:?}; update with `cargo install cargo-llvm-cov --locked --force`"
        ))
    })?;
    if found < MINIMUM_LLVM_COV_VERSION {
        return Err(CliError(format!(
            "cargo-llvm-cov {}.{}.{} is unsupported (minimum 0.6.0); update with `cargo install cargo-llvm-cov --locked --force`",
            found.0, found.1, found.2
        )));
    }
    Ok(())
}

fn check_llvm_cov_version() -> Result<(), CliError> {
    let output = Command::new("cargo")
        .args(["llvm-cov", "--version"])
        .output()
        .map_err(|err| {
            CliError(format!(
                "failed to inspect cargo-llvm-cov version ({err}); install with `cargo install cargo-llvm-cov`"
            ))
        })?;
    if !output.status.success() {
        return Err(CliError(
            "cargo llvm-cov --version failed; install or update with `cargo install cargo-llvm-cov --locked --force`".into(),
        ));
    }
    validate_version(&String::from_utf8_lossy(&output.stdout))
}

/// Check for the platform's browser opener before running coverage, so that
/// a missing opener produces a concrete diagnostic instead of a cryptic
/// non-zero exit status from cargo-llvm-cov. Does not contact the network.
fn ensure_opener_available() -> Result<(), CliError> {
    #[cfg(target_os = "windows")]
    let opener = "cmd.exe";
    #[cfg(target_os = "macos")]
    let opener = "open";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let opener = "xdg-open";

    let available = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path)
                .any(|directory| directory.join(opener).is_file())
        })
        .unwrap_or(false);
    if !available {
        return Err(CliError(format!(
            "--open needs {opener} in PATH on this platform; install the opener or omit --open and open target/llvm-cov/html/index.html manually"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(format: Format, fail_under: Option<f64>, open: bool) -> CoverageArgs {
        CoverageArgs {
            format,
            fail_under,
            open,
            dry_run: false,
            packages: Vec::new(),
            tests: Vec::new(),
            test_args: Vec::new(),
        }
    }

    fn out(verbosity: Verbosity, color: bool) -> Output {
        Output { verbosity, color }
    }

    fn normal() -> Output {
        out(Verbosity::Normal, true)
    }

    /// The forwarded arguments, for an invocation that must build.
    fn cargo_args(args: &CoverageArgs, out: Output) -> Vec<String> {
        Invocation::new(args, out).expect("valid flags").args
    }

    #[test]
    fn checks_tool_version_before_running_coverage() {
        assert_eq!(parse_version("cargo-llvm-cov 0.6.24"), Some((0, 6, 24)));
        assert_eq!(parse_version("cargo-llvm-cov 0.6.0-beta.1"), Some((0, 6, 0)));
        assert!(validate_version("cargo-llvm-cov 0.5.99").is_err());
        assert!(validate_version("not a version").is_err());
        assert!(validate_version("cargo-llvm-cov 0.6.0").is_ok());
    }

    #[test]
    fn forwards_packages_tests_and_test_harness_args() {
        let mut request = args(Format::Text, None, false);
        request.packages = vec!["contract".into(), "sdk".into()];
        request.tests = vec!["integration".into()];
        request.test_args = vec!["name_filter".into(), "--nocapture".into()];
        assert_eq!(
            cargo_args(&request, normal()),
            [
                "llvm-cov", "test", "--package", "contract", "--package", "sdk",
                "--test", "integration", "--", "name_filter", "--nocapture"
            ]
        );
    }

    #[test]
    fn fail_under_is_forwarded_as_fail_under_lines() {
        let cargo = cargo_args(&args(Format::Text, Some(80.0), false), normal());
        assert_eq!(cargo, ["llvm-cov", "test", "--fail-under-lines", "80"]);

        let cargo = cargo_args(&args(Format::Lcov, Some(92.5), false), normal());
        assert!(
            cargo.ends_with(&["--fail-under-lines".to_string(), "92.5".to_string()]),
            "{cargo:?}"
        );
    }

    #[test]
    fn no_threshold_means_no_fail_under_flag() {
        let cargo = cargo_args(&args(Format::Html, None, true), normal());
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

    #[test]
    fn each_format_maps_to_its_cargo_llvm_cov_flags() {
        let cases: [(Format, &[&str]); 3] = [
            (Format::Text, &["llvm-cov", "test"]),
            (
                Format::Lcov,
                &["llvm-cov", "test", "--lcov", "--output-path", "lcov.info"],
            ),
            (Format::Html, &["llvm-cov", "test", "--html"]),
        ];
        for (format, expected) in cases {
            assert_eq!(cargo_args(&args(format, None, false), normal()), expected);
        }
    }

    #[test]
    fn open_implies_html_from_the_default_format() {
        let cargo = cargo_args(&args(Format::Text, None, true), normal());
        assert_eq!(cargo, ["llvm-cov", "test", "--html", "--open"]);
    }

    #[test]
    fn open_with_lcov_is_rejected_instead_of_forwarding_conflicting_flags() {
        let err = Invocation::new(&args(Format::Lcov, None, true), normal())
            .expect_err("--lcov and --html conflict in cargo-llvm-cov");
        assert!(
            err.0.starts_with("--open only applies to the HTML report"),
            "{err}"
        );
    }

    #[test]
    fn all_flags_combine_in_a_stable_order() {
        let invocation = Invocation::new(
            &args(Format::Html, Some(75.0), true),
            out(Verbosity::Verbose, false),
        )
        .expect("valid flags");
        assert_eq!(
            invocation,
            Invocation {
                program: "cargo",
                args: [
                    "llvm-cov",
                    "test",
                    "--html",
                    "--open",
                    "--fail-under-lines",
                    "75",
                    "--color",
                    "never",
                    "--verbose",
                ]
                .map(String::from)
                .to_vec(),
                envs: vec![("CARGO_TERM_COLOR", "never"), ("NO_COLOR", "1")],
            }
        );
    }

    #[test]
    fn color_leaves_the_child_environment_alone() {
        let invocation = Invocation::new(&args(Format::Text, None, false), normal()).unwrap();
        assert!(invocation.envs.is_empty());
    }

    #[test]
    fn command_carries_the_program_args_and_envs() {
        let invocation = Invocation::new(
            &args(Format::Lcov, Some(80.0), false),
            out(Verbosity::Normal, false),
        )
        .unwrap();
        let cmd = invocation.command();

        assert_eq!(cmd.get_program(), "cargo");
        let forwarded: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(forwarded, invocation.args);
        let envs: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap(), v.and_then(|v| v.to_str())))
            .collect();
        assert_eq!(
            envs,
            [("CARGO_TERM_COLOR", Some("never")), ("NO_COLOR", Some("1"))]
        );
    }

    #[test]
    fn display_is_the_shell_command_dry_run_prints() {
        let plain = Invocation::new(&args(Format::Lcov, Some(80.0), false), normal()).unwrap();
        assert_eq!(
            plain.to_string(),
            "cargo llvm-cov test --lcov --output-path lcov.info --fail-under-lines 80"
        );

        let no_color = Invocation::new(
            &args(Format::Text, None, false),
            out(Verbosity::Quiet, false),
        )
        .unwrap();
        assert_eq!(
            no_color.to_string(),
            "CARGO_TERM_COLOR=never NO_COLOR=1 cargo llvm-cov test --color never --quiet"
        );
    }
}
