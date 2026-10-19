//! README runnable snippets (#214): every user-facing command and Rust
//! example in `README.md` is verified as an executable example.
//!
//! Bounded scope (documented here so future edits stay green):
//! - The `## Example` Rust block is tagged ` ```rust `, so it is compiled
//!   and run as a doctest through `crates/testkit/src/lib.rs`
//!   (`#![doc = include_str!("../../../README.md")]`). The test below pins
//!   the fence language and the doctest markers; `cargo test --doc` runs it.
//! - `sh` blocks that need no network, no `cargo-llvm-cov`, and no
//!   prebuilt `.wasm` are executed here via the real binary
//!   (`--help`, `--dry-run`, and the documented error shapes). They assert
//!   the shared error conventions: exit status 1 plus a single
//!   `error: <message>` line on stderr.
//! - `sh` blocks that inherently need the network (e.g.
//!   `cargo install ... --locked` from crates.io), a browser opener, a
//!   compiled `.wasm`, or `cargo-llvm-cov` are *not* executed. For those,
//!   this file asserts only that every `soroban-testkit` invocation names
//!   a real subcommand with real flags (checked against `--help`), so the
//!   snippet cannot drift from the CLI. The install path itself is covered
//!   by `scripts/smoke_test_cli_install.sh` (`make smoke-test-install`).
//! - `text` blocks are illustrative output or pseudo-code and are never
//!   compiled; `json` blocks are schema examples validated by the CLI's
//!   own `validate_json_summary` unit tests, not executed here.

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

fn read(rel: &str) -> String {
    let path = workspace_root().join(rel);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// `(fence language, body)` for every fenced block in `README.md`.
fn fenced_blocks(readme: &str) -> Vec<(String, String)> {
    // `split("```")` yields [pre, fence+body, post, fence+body, post, ...];
    // every odd index is one fence+body chunk.
    let chunks: Vec<&str> = readme.split("```").collect();
    let mut out = Vec::new();
    let mut i = 1;
    while i < chunks.len() {
        let chunk = chunks[i];
        let (lang, body) = match chunk.find('\n') {
            Some(pos) => (
                chunk[..pos].trim().to_string(),
                chunk[pos + 1..].to_string(),
            ),
            None => (chunk.trim().to_string(), String::new()),
        };
        out.push((lang, body));
        i += 2;
    }
    out
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
        .args(args)
        .current_dir(workspace_root())
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run soroban-testkit")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

// ---- Rust example -----------------------------------------------------------

#[test]
fn readme_rust_example_is_an_explicit_doctest() {
    let readme = read("README.md");

    // The `## Example` section must contain exactly one fenced block and it
    // must be tagged `rust` so rustdoc compiles and runs it. A bare fence
    // also defaults to Rust, but the explicit tag documents the intent and
    // keeps future edits from accidentally opting out.
    let section = readme
        .split("## Example")
        .nth(1)
        .expect("README.md must have a `## Example` section")
        .split("\n## ")
        .next()
        .unwrap();
    assert!(
        section.contains("```rust"),
        "`## Example` Rust block must be fenced as ```rust so it runs as a doctest"
    );

    // The doctest mechanism: lib.rs includes the whole README as crate docs.
    let lib_rs = read("crates/testkit/src/lib.rs");
    assert!(
        lib_rs.contains("#![doc = include_str!(\"../../../README.md\")]"),
        "crates/testkit/src/lib.rs must include README.md so `cargo test --doc` executes its Rust blocks"
    );

    // Pin the doctest markers the README relies on: an explicit `fn main`
    // hidden with `#` (top-level contract items live outside it) plus the
    // network-free testkit surface the example exercises.
    let blocks = fenced_blocks(&readme);
    let rust_blocks: Vec<&String> = blocks
        .iter()
        .filter(|(lang, _)| lang == "rust")
        .map(|(_, body)| body)
        .collect();
    assert!(
        !rust_blocks.is_empty(),
        "README.md must contain at least one ```rust block"
    );
    let example = rust_blocks[0];
    for marker in [
        "# fn main()",
        "TestEnv::new()",
        "env.token()",
        "events_during",
        "assert_emitted",
        "Conservation",
        "assert_holds()",
        "EscrowClient::new",
    ] {
        assert!(
            example.contains(marker),
            "README Rust example must contain `{marker}`"
        );
    }
}

#[test]
fn readme_has_no_untagged_fences() {
    // Every fence must declare its language (`rust`, `sh`, `text`, `json`,
    // ...). An untagged fence defaults to Rust in rustdoc, which would
    // silently turn shell transcripts or pseudo-code into failing doctests
    // (or silently run something meant as illustration). Pin the allowlist
    // so new snippets opt into verification explicitly.
    let readme = read("README.md");
    for (lang, _) in fenced_blocks(&readme) {
        assert!(
            ["rust", "sh", "text", "json"].contains(&lang.as_str()),
            "README.md has a fenced block with unexpected/missing language `{lang}`; \
             use ```rust for runnable Rust, ```sh for commands, ```text for output/pseudo-code, ```json for schema"
        );
    }
}

// ---- Shell snippets: static checks ------------------------------------------

/// Every `soroban-testkit ...` command line mentioned in `sh` blocks,
/// stripped of `$` prompts, line continuations, comments, and output lines.
fn readme_cli_invocations() -> Vec<String> {
    let readme = read("README.md");
    let mut out = Vec::new();
    for (lang, body) in fenced_blocks(&readme) {
        if lang != "sh" {
            continue;
        }
        // Join backslash continuations before splitting into lines.
        let joined = body.replace("\\\n", " ");
        for line in joined.lines() {
            let mut line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            line = line.trim_start_matches('$').trim();
            if line.starts_with("error:") || line == "..." || line.starts_with("...") {
                continue;
            }
            if line.starts_with("soroban-testkit") {
                // Strip trailing comments (`  # comment`) and inline output.
                let code = match line.find("  #") {
                    Some(pos) => line[..pos].trim(),
                    None => line,
                };
                // Skip illustrative output lines that start with the binary
                // name only inside comments (already handled); keep the rest.
                if code.starts_with("soroban-testkit") {
                    out.push(code.to_string());
                }
            }
        }
    }
    out
}

#[test]
fn readme_sh_blocks_use_only_supported_commands() {
    // Non-`soroban-testkit` shell lines must come from a small allowlist of
    // standard tools and shell builtins, so a snippet never depends on an
    // undocumented helper. `$VAR=...`, assignments, and redirections are
    // shell syntax, not commands.
    let readme = read("README.md");
    let allowed_first: &[&str] = &[
        "cargo",
        "jq",
        "mkdir",
        "cat",
        "rm",
        "TMP_DIR=",
        "CARGO_INSTALL_ROOT=",
    ];
    for (lang, body) in fenced_blocks(&readme) {
        if lang != "sh" {
            continue;
        }
        let joined = body.replace("\\\n", " ");
        for line in joined.lines() {
            let line = line.trim().trim_start_matches('$').trim();
            if line.is_empty()
                || line.starts_with('#')
                || line.starts_with("error:")
                || line.starts_with("soroban-testkit")
                || line.starts_with('"')
                || line.starts_with('\'')
                || line == "..."
                || line.starts_with("...")
                || line.contains('=')
            {
                continue;
            }
            let first = line.split_whitespace().next().unwrap();
            assert!(
                allowed_first
                    .iter()
                    .any(|a| first == *a || line.starts_with(a)),
                "README sh snippet uses unsupported command `{line}`; \
                 keep snippets to cargo/soroban-testkit/jq/mkdir/cat/rm/shell assignments"
            );
        }
    }
}

#[test]
fn readme_cli_invocations_name_real_subcommands_and_flags() {
    // Every documented invocation must name a real subcommand; every long
    // flag it uses must appear in that subcommand's `--help`. This keeps
    // usage snippets executable in principle without running the
    // network/browser/wasm-dependent ones here.
    let invocations = readme_cli_invocations();
    assert!(
        invocations.len() >= 8,
        "expected at least 8 documented soroban-testkit invocations, found {invocations:?}"
    );

    let top_help = stdout(&run(&["--help"]));
    for sub in ["coverage", "limits", "audit"] {
        assert!(
            top_help.contains(sub),
            "top-level --help must list `{sub}` (README documents it):\n{top_help}"
        );
    }

    let help_for = |sub: &str| stdout(&run(&[sub, "--help"]));
    let helps = [
        ("coverage", help_for("coverage")),
        ("limits", help_for("limits")),
        ("audit", help_for("audit")),
    ];

    for invocation in &invocations {
        let parts: Vec<&str> = invocation.split_whitespace().collect();
        assert_eq!(parts[0], "soroban-testkit", "{invocation}");
        // Top-level flags (`soroban-testkit --help`) name no subcommand;
        // they are covered by `readme_help_snippet_runs`.
        let Some(sub) = parts
            .iter()
            .find(|p| ["coverage", "limits", "audit"].contains(p))
        else {
            assert!(
                parts
                    .iter()
                    .any(|p| ["--help", "-h", "--version", "-V"].contains(p)),
                "invocation names no known subcommand: {invocation}"
            );
            continue;
        };
        let help = &helps.iter().find(|(name, _)| name == sub).unwrap().1;
        for token in parts.iter().skip(1) {
            if let Some(flag) = token.strip_prefix("--") {
                let flag = flag.trim_end_matches(',');
                // `--` separator and values (paths, numbers) are not flags.
                if flag.is_empty() || flag == "-" {
                    continue;
                }
                assert!(
                    help.contains(&format!("--{flag}")),
                    "README invocation `{invocation}` uses --{flag}, missing from `soroban-testkit {sub} --help`"
                );
            }
        }
    }
}

// ---- Shell snippets: executed network-free -------------------------------

#[test]
fn readme_help_snippet_runs() {
    // `soroban-testkit --help` (README "Verify the installation").
    let output = run(&["--help"]);
    assert!(output.status.success(), "stderr: {}", stderr(&output));
    let help = stdout(&output);
    for sub in ["coverage", "limits", "audit"] {
        assert!(help.contains(sub), "missing `{sub}` in:\n{help}");
    }
}

#[test]
fn readme_coverage_dry_run_snippets_need_no_toolchain_or_network() {
    // README: "`--dry-run` ... needs neither cargo nor cargo-llvm-cov
    // installed". Run with an empty PATH so no toolchain can be found; the
    // command must still print the cargo invocation it would run.
    for args in [
        vec![
            "coverage",
            "--format",
            "lcov",
            "--fail-under",
            "80",
            "--dry-run",
        ],
        vec![
            "coverage",
            "--format",
            "json",
            "--fail-under",
            "80",
            "--dry-run",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
            .args(&args)
            .current_dir(workspace_root())
            .env("NO_COLOR", "1")
            .env("PATH", "")
            .output()
            .expect("failed to run soroban-testkit");
        assert!(
            output.status.success(),
            "{args:?} failed with empty PATH: {}",
            stderr(&output)
        );
        let out = stdout(&output);
        assert!(
            out.contains("cargo llvm-cov test"),
            "{args:?} --dry-run must print the cargo command, got: {out}"
        );
        assert!(
            out.contains("--fail-under-lines 80"),
            "{args:?} must forward the threshold, got: {out}"
        );
    }
}

#[test]
fn readme_failure_examples_match_the_shared_error_conventions() {
    // README "Failure examples" documents exit status 1 plus a single
    // `error: <message>` line on stderr. Reproduce two of them network-free
    // and assert the exact prefixes the README shows, so the docs cannot
    // drift from `CliError`'s conventions. (The `limits` unknown-parameter
    // and `coverage` llvm-cov-missing shapes are covered by
    // `error_output.rs` and `coverage` unit tests against the same strings.)

    // `soroban-testkit audit ./scr` — PATH doesn't exist.
    let output = run(&["audit", "./scr"]);
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "stderr: {err}");
    assert!(
        err.starts_with("error: cannot audit `./scr`: "),
        "README failure shape drifted: {err}"
    );
    assert!(
        err.contains("pass a contract crate's source directory"),
        "README failure shape drifted: {err}"
    );

    // `soroban-testkit audit ./src --strict` with findings exits non-zero
    // with the README's `N audit finding(s) (--strict)` suffix. Run it
    // against this workspace's own `crates/cli/src`, which has findings
    // under the three-rule linter, rather than a fixture.
    let output = run(&["audit", "crates/cli/src", "--strict"]);
    let err = stderr(&output);
    assert_eq!(output.status.code(), Some(1), "stderr: {err}");
    assert!(
        err.trim_end().ends_with("audit finding(s) (--strict)"),
        "README --strict shape drifted: {err}"
    );
}

#[test]
fn readme_json_schema_example_matches_the_cli_contract() {
    // The README's ```json block documents the `--format json` file shape
    // (`llvm.coverage.json.export` with totals + per-file summaries). Assert
    // the documented `type` marker and the output path constant agree with
    // the CLI source, so the schema example stays executable in the sense
    // that `jq '.data[0].totals.lines.percent' coverage.json` (the very next
    // README snippet) addresses a field the CLI really writes.
    let readme = read("README.md");
    let json_block = fenced_blocks(&readme)
        .into_iter()
        .find(|(lang, _)| lang == "json")
        .map(|(_, body)| body)
        .expect("README.md must contain a ```json schema example");
    for marker in [
        "\"type\": \"llvm.coverage.json.export\"",
        "\"totals\"",
        "\"lines\"",
        "\"percent\"",
    ] {
        assert!(
            json_block.contains(marker),
            "README json example must contain `{marker}`:\n{json_block}"
        );
    }
    let coverage_src = read("crates/cli/src/commands/coverage.rs");
    assert!(
        coverage_src.contains("llvm.coverage.json.export"),
        "coverage.rs must emit the export type the README documents"
    );
    assert!(
        coverage_src.contains("coverage.json"),
        "coverage.rs must write the `coverage.json` path the README's jq snippet reads"
    );
}
