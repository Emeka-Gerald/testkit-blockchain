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
    /// A machine-readable summary written to `coverage.json`, for CI.
    ///
    /// The file is cargo-llvm-cov's `--json --summary-only` export (LLVM's
    /// `llvm.coverage.json.export` format: totals and per-file summaries,
    /// no per-line data). It is checked against that schema after the run;
    /// see [`validate_json_summary`].
    Json,
}

/// Where `--format json` writes its summary, relative to the working
/// directory (as `--format lcov` writes `lcov.info`).
pub const JSON_OUTPUT_PATH: &str = "coverage.json";

/// The `type` field of an LLVM coverage export, which cargo-llvm-cov's
/// `--json` output passes through unchanged.
const LLVM_EXPORT_TYPE: &str = "llvm.coverage.json.export";

/// Arguments for `soroban-testkit coverage`.
#[derive(Args)]
#[command(after_help = "EXAMPLES:\n    \
    soroban-testkit coverage --format html --open\n    \
    soroban-testkit coverage --format lcov\n    \
    soroban-testkit coverage --format json --fail-under 80\n    \
    soroban-testkit coverage --fail-under 80\n    \
    soroban-testkit coverage --format lcov --fail-under 80 --dry-run")]
pub struct CoverageArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Text)]
    format: Format,
    /// Write the report to this path (or directory for HTML). Defaults to
    /// `lcov.info`, `coverage.json`, or `target/llvm-cov/html` by format.
    #[arg(long, value_name = "PATH")]
    output: Option<std::path::PathBuf>,
    /// Exit 1 if line coverage is below PCT percent (0-100). Forwarded to
    /// cargo-llvm-cov as --fail-under-lines. Default: no threshold
    #[arg(long, value_name = "PCT", verbatim_doc_comment)]
    fail_under: Option<f64>,
    /// Open the HTML report in a browser after generating it (implies
    /// --format html)
    #[arg(verbatim_doc_comment)]
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
    let output = invocation.command().output().map_err(|err| {
        CliError(format!(
            "failed to run `cargo llvm-cov` ({err}); is cargo-llvm-cov installed? \
             try `cargo install cargo-llvm-cov`"
        ))
    })?;

    if !output.status.success() {
        if args.open {
            return Err(CliError(format!(
                "coverage run or HTML browser launch failed (cargo llvm-cov exited with {status}); try without --open, then open target/llvm-cov/html/index.html manually; check your browser configuration"
            )));
        }
        return Err(CliError(format!(
            "coverage run failed (cargo llvm-cov exited with {status})"
        )));
    }

    if matches!(args.format, Format::Json) {
        let path = args.output.as_deref().unwrap_or(std::path::Path::new(JSON_OUTPUT_PATH));
        let json = std::fs::read_to_string(path).map_err(|err| {
            CliError(format!(
                "cargo llvm-cov succeeded but {} could not be read: {err}", path.display()
            ))
        })?;
        let lines = validate_json_summary(&json).map_err(|err| {
            CliError(format!(
                "{} is not a valid coverage summary: {err}", path.display()
            ))
        })?;
        out.note(format!(
            "wrote {}: line coverage {:.2}% ({}/{} lines)", path.display(), lines.percent,
            lines.covered, lines.count
        ));
    }

    Ok(())
}

/// Totals for one coverage metric (lines, functions or regions) from a
/// coverage JSON summary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricTotals {
    /// Number of instrumented items.
    pub count: u64,
    /// Number of those items executed at least once.
    pub covered: u64,
    /// `covered / count` as a percentage, as reported by LLVM.
    pub percent: f64,
}

/// Checks that `json` has the shape `--format json` promises to CI
/// integrations and returns its line totals.
///
/// The required schema is the stable subset of LLVM's coverage export
/// (format version `2.x`):
///
/// ```text
/// {
///   "type": "llvm.coverage.json.export",
///   "version": "2.<minor>.<patch>",
///   "data": [{
///     "totals": {
///       "lines":     { "count": <u64>, "covered": <u64>, "percent": <number> },
///       "functions": { "count": <u64>, "covered": <u64>, "percent": <number> },
///       "regions":   { "count": <u64>, "covered": <u64>, "percent": <number> }
///     },
///     "files": [ ... ]
///   }]
/// }
/// ```
///
/// with `covered <= count` and `percent` in `0..=100`. Other fields (e.g.
/// `branches`, per-file `summary`) are allowed and ignored. Only the first
/// `data` entry is read; cargo-llvm-cov emits exactly one.
pub fn validate_json_summary(json: &str) -> Result<MetricTotals, String> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|err| format!("invalid JSON: {err}"))?;

    match root.get("type").and_then(|v| v.as_str()) {
        Some(LLVM_EXPORT_TYPE) => {}
        Some(other) => {
            return Err(format!(
                "`type` is {other:?}, expected {LLVM_EXPORT_TYPE:?}"
            ))
        }
        None => return Err("missing string field `type`".to_string()),
    }
    let version = root
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or("missing string field `version`")?;
    if version.split('.').next() != Some("2") {
        return Err(format!(
            "unsupported export `version` {version:?}, expected 2.x"
        ));
    }

    let entry = root
        .get("data")
        .and_then(|v| v.as_array())
        .and_then(|data| data.first())
        .ok_or("`data` must be a non-empty array")?;
    if !entry.get("files").is_some_and(|v| v.is_array()) {
        return Err("missing array field `data[0].files`".to_string());
    }
    let totals = entry
        .get("totals")
        .ok_or("missing object field `data[0].totals`")?;

    let lines = metric_totals(totals, "lines")?;
    metric_totals(totals, "functions")?;
    metric_totals(totals, "regions")?;
    Ok(lines)
}

/// Parses and checks `data[0].totals.<metric>`.
fn metric_totals(totals: &serde_json::Value, metric: &str) -> Result<MetricTotals, String> {
    let path = format!("data[0].totals.{metric}");
    let obj = totals
        .get(metric)
        .ok_or_else(|| format!("missing object field `{path}`"))?;
    let uint = |field: &str| {
        obj.get(field)
            .and_then(|v| v.as_u64())
            .ok_or_else(|| format!("`{path}.{field}` must be a non-negative integer"))
    };
    let count = uint("count")?;
    let covered = uint("covered")?;
    let percent = obj
        .get("percent")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| format!("`{path}.percent` must be a number"))?;
    if covered > count {
        return Err(format!(
            "`{path}.covered` ({covered}) exceeds `{path}.count` ({count})"
        ));
    }
    if !(0.0..=100.0).contains(&percent) {
        return Err(format!("`{path}.percent` ({percent}) is outside 0..=100"));
    }
    Ok(MetricTotals {
        count,
        covered,
        percent,
    })
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
            Format::Text => {
                if let Some(path) = &args.output {
                    push("--output-path");
                    push(path.to_str().unwrap_or("coverage.txt"));
                }
            }
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
                push(args.output.as_deref().unwrap_or(std::path::Path::new("lcov.info")).to_str().unwrap_or("lcov.info"));
            }
            Format::Html => {
                push("--html");
                if let Some(path) = &args.output {
                    push("--output-dir");
                    push(path.to_str().unwrap_or("target/llvm-cov/html"));
                }
            }
            Format::Json => {
                push("--json");
                push("--summary-only");
                push("--output-path");
                push(args.output.as_deref().unwrap_or(std::path::Path::new(JSON_OUTPUT_PATH)).to_str().unwrap_or(JSON_OUTPUT_PATH));
            }
            Format::Html => push("--html"),
            Format::Json => {
                push("--json");
                push("--summary-only");
                push("--output-path");
                push(JSON_OUTPUT_PATH);
            }
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
        .map(|path| std::env::split_paths(&path).any(|directory| directory.join(opener).is_file()))
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
            output: None,
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
        assert_eq!(
            parse_version("cargo-llvm-cov 0.6.0-beta.1"),
            Some((0, 6, 0))
        );
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
                "llvm-cov",
                "test",
                "--package",
                "contract",
                "--package",
                "sdk",
                "--test",
                "integration",
                "--",
                "name_filter",
                "--nocapture"
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
    fn json_format_writes_a_summary_only_export() {
        let cargo = cargo_args(
            &args(Format::Json, Some(80.0), false),
            out(Verbosity::Normal, true),
        );
        assert_eq!(
            cargo,
            [
                "llvm-cov",
                "test",
                "--json",
                "--summary-only",
                "--output-path",
                "coverage.json",
                "--fail-under-lines",
                "80",
            ]
        );
    }

    #[test]
    fn explicit_output_path_is_forwarded_for_each_file_format() {
        for (format, flag) in [(Format::Text, "--output-path"), (Format::Lcov, "--output-path"), (Format::Json, "--output-path")] {
            let mut request = args(format, None, false);
            request.output = Some("reports/result.out".into());
            let cargo = cargo_args(&request, normal());
            assert!(cargo.windows(2).any(|pair| pair == [flag.to_string(), "reports/result.out".to_string()]), "{cargo:?}");
        }
        let mut request = args(Format::Html, None, false);
        request.output = Some("reports/html".into());
        let cargo = cargo_args(&request, normal());
        assert!(cargo.windows(2).any(|pair| pair == ["--output-dir".to_string(), "reports/html".to_string()]));
    }

    /// A trimmed `cargo llvm-cov test --json --summary-only` export.
    const SAMPLE: &str = r#"{
        "data": [{
            "files": [{
                "filename": "src/lib.rs",
                "summary": {"lines": {"count": 40, "covered": 34, "percent": 85.0}}
            }],
            "totals": {
                "branches": {"count": 0, "covered": 0, "notcovered": 0, "percent": 0},
                "functions": {"count": 10, "covered": 9, "percent": 90.0},
                "lines": {"count": 40, "covered": 34, "percent": 85.0},
                "regions": {"count": 60, "covered": 48, "notcovered": 12, "percent": 80.0}
            }
        }],
        "type": "llvm.coverage.json.export",
        "version": "2.0.1"
    }"#;

    const LINES: &str = r#""lines": {"count": 40, "covered": 34, "percent": 85.0}"#;

    #[test]
    fn valid_summary_yields_line_totals() {
        assert_eq!(
            validate_json_summary(SAMPLE),
            Ok(MetricTotals {
                count: 40,
                covered: 34,
                percent: 85.0,
            })
        );
    }

    #[test]
    fn each_format_maps_to_its_cargo_llvm_cov_flags() {
        let cases: [(Format, &[&str]); 4] = [
            (Format::Text, &["llvm-cov", "test"]),
            (
                Format::Lcov,
                &["llvm-cov", "test", "--lcov", "--output-path", "lcov.info"],
            ),
            (Format::Html, &["llvm-cov", "test", "--html"]),
            (
                Format::Json,
                &[
                    "llvm-cov",
                    "test",
                    "--json",
                    "--summary-only",
                    "--output-path",
                    "coverage.json",
                ],
            ),
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
    fn zero_instrumented_lines_is_valid() {
        let json = SAMPLE.replace(
            LINES,
            r#""lines": {"count": 0, "covered": 0, "percent": 0}"#,
        );
        assert_eq!(validate_json_summary(&json).map(|lines| lines.count), Ok(0));
    }

    fn rejection(json: &str) -> String {
        validate_json_summary(json).expect_err("schema check should fail")
    }

    #[test]
    fn malformed_envelope_is_rejected() {
        assert!(rejection("not json").starts_with("invalid JSON"));
        assert!(rejection("{}").contains("`type`"));
        assert!(
            rejection(&SAMPLE.replace("llvm.coverage.json.export", "lcov")).contains("\"lcov\"")
        );
        assert!(rejection(&SAMPLE.replace("\"2.0.1\"", "\"3.0.0\"")).contains("2.x"));
        assert!(rejection(&SAMPLE.replace("\"data\"", "\"dat\"")).contains("non-empty"));
        assert!(rejection(&SAMPLE.replace("\"files\"", "\"filez\"")).contains("data[0].files"));
        assert!(rejection(&SAMPLE.replace("\"totals\"", "\"tot\"")).contains("data[0].totals"));
    }

    #[test]
    fn malformed_totals_are_rejected_with_the_offending_field() {
        assert!(rejection(&SAMPLE.replace("\"functions\"", "\"fns\""))
            .contains("data[0].totals.functions"));
        assert!(rejection(&SAMPLE.replace(
            LINES,
            r#""lines": {"count": -1, "covered": 34, "percent": 85.0}"#
        ))
        .contains("data[0].totals.lines.count"));
        assert!(rejection(&SAMPLE.replace(
            LINES,
            r#""lines": {"count": 40, "covered": 41, "percent": 85.0}"#
        ))
        .contains("exceeds"));
        assert!(rejection(&SAMPLE.replace(
            LINES,
            r#""lines": {"count": 40, "covered": 34, "percent": 185.0}"#
        ))
        .contains("outside 0..=100"));
        assert!(
            rejection(&SAMPLE.replace("\"percent\": 80.0", "\"percent\": \"80\""))
                .contains("data[0].totals.regions.percent")
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
