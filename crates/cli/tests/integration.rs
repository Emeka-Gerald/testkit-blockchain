//! Integration tests against `examples/vault`, run as a subprocess of the
//! real `soroban-testkit` binary — exercising the CLI the way a user
//! would, not just its internal functions.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .expect("failed to run soroban-testkit")
}

fn run_with_stdin(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
        .args(args)
        .current_dir(workspace_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run soroban-testkit");
    child
        .stdin
        .take()
        .expect("child stdin")
        .write_all(stdin.as_bytes())
        .expect("failed to write stdin");
    child
        .wait_with_output()
        .expect("failed to wait on soroban-testkit")
}

#[test]
fn no_subcommand_shows_concise_usage_and_exits_non_zero() {
    let output = run(&[]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("Usage:"), "{stderr}");
    assert!(stderr.contains("soroban-testkit"), "{stderr}");
    // Concise: a missing-subcommand error, not a panic or backtrace.
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!stderr.contains("RUST_BACKTRACE"), "{stderr}");
}

#[test]
fn help_invocation_succeeds_and_lists_subcommands() {
    let output = run(&["--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("Usage:"), "{stdout}");
    assert!(stdout.contains("coverage"), "{stdout}");
    assert!(stdout.contains("limits"), "{stdout}");
    assert!(stdout.contains("audit"), "{stdout}");
}

#[test]
fn audit_reports_findings_against_vault() {
    let output = run(&["audit", "examples/vault/src"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("missing-require-auth"), "{stdout}");
    assert!(stdout.contains("missing-ttl-bump"), "{stdout}");
    assert!(stdout.contains("unchecked-i128-arithmetic"), "{stdout}");
    assert!(stdout.contains("not a security product"), "{stdout}");
}

#[test]
fn audit_strict_exits_non_zero_on_findings() {
    let output = run(&["audit", "examples/vault/src", "--strict"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("audit finding(s)"), "{stderr}");
}

#[test]
fn audit_reports_no_findings_on_a_clean_directory() {
    let output = run(&["audit", "crates/cli/src/commands/mod.rs"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("no findings"), "{stdout}");
    assert!(
        stdout.contains("a missing finding is not a security guarantee"),
        "a clean run must still carry the disclaimer: {stdout}"
    );
}

#[test]
fn audit_reads_source_from_stdin_when_path_is_dash() {
    let src = r#"
        fn transfer(to: Address, amount: i128) {
            let total = amount + 1;
        }
    "#;
    let output = run_with_stdin(&["audit", "-"], src);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("missing-require-auth"), "{stdout}");
    assert!(stdout.contains("unchecked-i128-arithmetic"), "{stdout}");
    assert!(stdout.contains("<stdin>"), "{stdout}");
}

#[test]
fn audit_stdin_reports_no_findings_on_empty_input() {
    let output = run_with_stdin(&["audit", "-"], "");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("no findings"), "{stdout}");
}

#[test]
fn audit_stdin_reports_no_findings_on_invalid_input() {
    let output = run_with_stdin(&["audit", "-"], "fn {{{ not valid rust");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("no findings"), "{stdout}");
}

#[test]
fn audit_json_declares_schema_version_and_findings() {
    let output = run(&["audit", "examples/vault/src", "--json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.starts_with("{\"schema_version\":1,"),
        "schema_version must lead the document: {stdout}"
    );
    assert!(stdout.contains("\"finding_count\":"), "{stdout}");
    assert!(
        stdout.contains("\"rule\":\"missing-require-auth\""),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"disclaimer\":\"soroban-testkit audit"),
        "{stdout}"
    );
}

#[test]
fn audit_json_on_a_clean_file_is_an_empty_findings_list() {
    let output = run(&["audit", "crates/cli/src/commands/mod.rs", "--json"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout
            .trim()
            .ends_with(",\"finding_count\":0,\"findings\":[]}"),
        "{stdout}"
    );
}

#[test]
fn limits_finds_the_real_mainnet_write_ceiling_for_batch_payout() {
    let build = Command::new("cargo")
        .args([
            "build",
            "-p",
            "vault",
            "--target",
            "wasm32v1-none",
            "--release",
        ])
        .current_dir(workspace_root())
        .status()
        .expect("failed to build vault.wasm");
    assert!(build.success());

    let wasm = workspace_root().join("target/wasm32v1-none/release/vault.wasm");
    let output = run(&[
        "limits",
        "--contract",
        wasm.to_str().unwrap(),
        "--fn",
        "batch_payout",
        "--ramp",
        "recipients",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("last successful recipients"), "{stdout}");
    assert!(stdout.contains("instructions:"), "{stdout}");
    assert!(stdout.contains("memory bytes:"), "{stdout}");
}

#[test]
fn limits_reports_an_actionable_error_for_an_unknown_function() {
    let wasm = workspace_root().join("target/wasm32v1-none/release/vault.wasm");
    if !wasm.exists() {
        // Built by limits_finds_the_real_mainnet_write_ceiling_for_batch_payout;
        // skip if tests ran in isolation without it.
        return;
    }
    let output = run(&[
        "limits",
        "--contract",
        wasm.to_str().unwrap(),
        "--fn",
        "does_not_exist",
        "--ramp",
        "x",
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("no function named"), "{stderr}");
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run soroban-testkit")
}

#[test]
fn audit_paths_are_relative_to_the_audited_root_from_any_working_directory() {
    let root = workspace_root();
    let vault_src = root.join("examples/vault/src");
    let from_workspace = run_in(&root, &["audit", "examples/vault/src", "--json"]);
    let from_crate = run_in(&root.join("examples/vault"), &["audit", "src", "--json"]);
    let absolute = run_in(
        &root.join("crates"),
        &["audit", vault_src.to_str().unwrap(), "--json"],
    );

    let stdout = String::from_utf8_lossy(&from_workspace.stdout);
    assert!(stdout.contains("\"file\":\"lib.rs\""), "{stdout}");
    assert!(!stdout.contains("examples/vault"), "{stdout}");
    assert_eq!(from_workspace.stdout, from_crate.stdout);
    assert_eq!(from_workspace.stdout, absolute.stdout);
}

#[test]
fn audit_text_output_uses_root_relative_paths() {
    let output = run_in(&workspace_root().join("examples/vault"), &["audit"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(
        stdout.lines().any(|l| l.starts_with("src/lib.rs:")),
        "{stdout}"
    );
    assert!(!stdout.contains("./"), "{stdout}");
}

#[test]
fn no_color_output_is_ansi_free() {
    for args in [
        &["--no-color", "audit", "examples/vault/src", "--strict"][..],
        &["audit", "examples/vault/src", "--no-color", "--json"][..],
        &["--no-color", "audit", "does/not/exist.rs", "--bogus"][..],
    ] {
        let output = run(args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stdout.contains('\x1b'), "{args:?} stdout: {stdout}");
        assert!(!stderr.contains('\x1b'), "{args:?} stderr: {stderr}");
    }
}

#[test]
fn no_color_env_var_keeps_errors_ansi_free() {
    let output = Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
        .args(["audit", "examples/vault/src", "--strict"])
        .current_dir(workspace_root())
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run soroban-testkit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("error: "), "{stderr}");
    assert!(!stderr.contains('\x1b'), "{stderr}");
}

#[test]
fn quiet_drops_advisory_notes_but_keeps_findings() {
    let output = run(&["-q", "audit", "examples/vault/src"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("missing-require-auth"), "{stdout}");
    assert!(!stdout.contains("not a security product"), "{stdout}");

    let clean = run(&["audit", "crates/cli/src/commands/mod.rs", "--quiet"]);
    assert!(clean.status.success());
    assert!(clean.stdout.is_empty(), "{:?}", clean.stdout);
}

#[test]
fn quiet_still_reports_errors_and_json() {
    let strict = run(&["--quiet", "audit", "examples/vault/src", "--strict"]);
    assert!(!strict.status.success());
    let stderr = String::from_utf8_lossy(&strict.stderr);
    assert!(stderr.contains("audit finding(s)"), "{stderr}");

    let json = run(&["--quiet", "audit", "examples/vault/src", "--json"]);
    let stdout = String::from_utf8_lossy(&json.stdout);
    assert!(stdout.starts_with("{\"schema_version\":1,"), "{stdout}");
}

#[test]
fn verbose_adds_diagnostics_on_stderr_only() {
    let normal = run(&["audit", "examples/vault/src"]);
    let verbose = run(&["--verbose", "audit", "examples/vault/src"]);
    let stderr = String::from_utf8_lossy(&verbose.stderr);

    assert!(verbose.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("scanning "), "{stderr}");
    assert!(stderr.contains("lib.rs"), "{stderr}");
    assert!(normal.stderr.is_empty());
    assert_eq!(normal.stdout, verbose.stdout);
}

#[test]
fn quiet_and_verbose_conflict() {
    let output = run(&["-q", "-v", "audit", "examples/vault/src"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
}

#[test]
fn coverage_help_includes_usage_examples() {
    let output = run(&["coverage", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("EXAMPLES"), "{stdout}");
    assert!(stdout.contains("--format html"), "{stdout}");
    assert!(stdout.contains("--format lcov"), "{stdout}");
    assert!(stdout.contains("--format json"), "{stdout}");
    assert!(stdout.contains("--fail-under"), "{stdout}");
    assert!(stdout.contains("--dry-run"), "{stdout}");
}

#[test]
fn limits_help_includes_usage_examples() {
    let output = run(&["limits", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("EXAMPLES"), "{stdout}");
    assert!(stdout.contains("--contract"), "{stdout}");
    assert!(stdout.contains("--fn"), "{stdout}");
    assert!(stdout.contains("--ramp"), "{stdout}");
}

#[test]
fn audit_help_includes_usage_examples() {
    let output = run(&["audit", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success(), "stdout: {stdout}");
    assert!(stdout.contains("EXAMPLES"), "{stdout}");
    assert!(stdout.contains("--strict"), "{stdout}");
    assert!(stdout.contains("--json"), "{stdout}");
}

// ---- #204: coverage argument construction without cargo-llvm-cov --------

/// Runs the binary with `PATH` pointing at a directory that does not
/// exist, so neither `cargo` nor `cargo-llvm-cov` can be found: anything
/// that passes here demonstrably needs no external tool.
fn run_without_tools(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
        .args(args)
        .current_dir(workspace_root())
        .env(
            "PATH",
            workspace_root().join("target/no-tools-on-this-path"),
        )
        .env_remove("NO_COLOR")
        .output()
        .expect("failed to run soroban-testkit")
}

fn dry_run(args: &[&str]) -> String {
    let output = run_without_tools(args);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{args:?}: stderr: {stderr}");
    assert!(stderr.is_empty(), "{args:?}: stderr: {stderr}");
    stdout
}

#[test]
fn coverage_dry_run_prints_the_forwarded_flags_without_cargo_llvm_cov() {
    let cases: &[(&[&str], &str)] = &[
        (&["coverage", "--dry-run"], "cargo llvm-cov test\n"),
        (
            &[
                "coverage",
                "--dry-run",
                "--format",
                "lcov",
                "--fail-under",
                "92.5",
            ],
            "cargo llvm-cov test --lcov --output-path lcov.info --fail-under-lines 92.5\n",
        ),
        (
            &["coverage", "--dry-run", "--format", "html", "--open"],
            "cargo llvm-cov test --html --open\n",
        ),
        (
            &["coverage", "--dry-run", "--open"],
            "cargo llvm-cov test --html --open\n",
        ),
        (
            &["-v", "coverage", "--dry-run", "--fail-under", "80"],
            "cargo llvm-cov test --fail-under-lines 80 --verbose\n",
        ),
        (
            &["--no-color", "-q", "coverage", "--dry-run"],
            "CARGO_TERM_COLOR=never NO_COLOR=1 cargo llvm-cov test --color never --quiet\n",
        ),
    ];
    for (args, expected) in cases {
        assert_eq!(dry_run(args), *expected, "{args:?}");
    }
}

#[test]
fn coverage_open_with_lcov_is_rejected_before_running_anything() {
    let output = run_without_tools(&["coverage", "--format", "lcov", "--open"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.starts_with("error: --open only applies to the HTML report"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn coverage_without_cargo_on_path_reports_how_to_install_it() {
    let output = run_without_tools(&["coverage", "--fail-under", "80"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.starts_with("error: failed to run `cargo llvm-cov`"),
        "{stderr}"
    );
    assert!(stderr.contains("cargo install cargo-llvm-cov"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}
