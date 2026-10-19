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
    soroban-testkit coverage --fail-under 80")]
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

    if matches!(args.format, Format::Json) {
        let json = std::fs::read_to_string(JSON_OUTPUT_PATH).map_err(|err| {
            CliError(format!(
                "cargo llvm-cov succeeded but {JSON_OUTPUT_PATH} could not be read: {err}"
            ))
        })?;
        let lines = validate_json_summary(&json).map_err(|err| {
            CliError(format!(
                "{JSON_OUTPUT_PATH} is not a valid coverage summary: {err}"
            ))
        })?;
        out.note(format!(
            "wrote {JSON_OUTPUT_PATH}: line coverage {:.2}% ({}/{} lines)",
            lines.percent, lines.covered, lines.count
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
}
