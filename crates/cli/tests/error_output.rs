//! Error output consistency (#202): every subcommand reports a failure as
//! exit status 1 plus a single `error: <message>` line on stderr, with no
//! Rust debug output, panic, or backtrace. See the conventions on
//! `CliError` in `src/commands/mod.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run soroban-testkit")
}

/// Shapes of raw `Debug` output that must never reach the user.
const DEBUG_MARKERS: &[&str] = &[
    "Os {",
    "kind:",
    " { ",
    "ScSpecType",
    "Box(",
    "\\\"",
    "panicked",
    "RUST_BACKTRACE",
];

/// Asserts `output` is a command failure in the shared format and returns
/// its message (the stderr text after `error: `).
fn assert_cli_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.starts_with("error: "), "{stderr}");
    assert!(stderr.ends_with('\n'), "{stderr:?}");
    assert_eq!(stderr.matches("error:").count(), 1, "{stderr}");
    assert!(!stderr.contains('\x1b'), "{stderr}");
    for marker in DEBUG_MARKERS {
        assert!(
            !stderr.contains(marker),
            "debug output {marker:?} in: {stderr}"
        );
    }
    stderr["error: ".len()..].trim_end().to_string()
}

#[test]
fn audit_of_a_missing_path_fails_instead_of_reporting_no_findings() {
    let output = run(&["audit", "does/not/exist"]);
    let message = assert_cli_error(&output);
    assert!(
        message.starts_with("cannot audit `does/not/exist`: "),
        "{message}"
    );
    assert!(
        message.contains("pass a contract crate's source directory"),
        "{message}"
    );
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn audit_strict_findings_use_the_shared_format() {
    let output = run(&["audit", "examples/vault/src", "--strict"]);
    let message = assert_cli_error(&output);
    assert!(
        message.ends_with("audit finding(s) (--strict)"),
        "{message}"
    );
}

#[test]
fn limits_with_a_missing_contract_names_the_path_and_the_fix() {
    let output = run(&[
        "limits",
        "--contract",
        "missing.wasm",
        "--fn",
        "batch_payout",
        "--ramp",
        "recipients",
    ]);
    let message = assert_cli_error(&output);
    assert!(
        message.starts_with("failed to read `missing.wasm`: "),
        "{message}"
    );
    assert!(message.contains("compiled contract .wasm"), "{message}");
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn limits_with_a_non_wasm_file_explains_what_to_pass() {
    let output = run(&[
        "limits",
        "--contract",
        "README.md",
        "--fn",
        "batch_payout",
        "--ramp",
        "recipients",
    ]);
    let message = assert_cli_error(&output);
    assert!(
        message.starts_with("`README.md` is not a valid WebAssembly module"),
        "{message}"
    );
    assert!(message.contains("wasm32v1-none"), "{message}");
}

#[test]
fn limits_with_an_unknown_ramp_parameter_lists_the_real_ones() {
    let wasm = workspace_root().join("target/wasm32v1-none/release/vault.wasm");
    if !wasm.exists() {
        // Built by integration.rs's batch_payout limits test; skip if tests
        // ran in isolation without it.
        return;
    }
    let output = run(&[
        "limits",
        "--contract",
        wasm.to_str().unwrap(),
        "--fn",
        "batch_payout",
        "--ramp",
        "nonexistent_param",
    ]);
    let message = assert_cli_error(&output);
    assert!(
        message.starts_with("`batch_payout` has no parameter named `nonexistent_param`"),
        "{message}"
    );
    assert!(
        message.ends_with("; its parameters are `recipients`, `amount`"),
        "{message}"
    );
}

#[test]
fn usage_errors_also_start_with_the_error_prefix() {
    for args in [
        &["audit", "--bogus"][..],
        &["limits", "--contract", "x.wasm"][..],
    ] {
        let output = run(args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(stderr.starts_with("error: "), "{args:?}: {stderr}");
        assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    }
}
