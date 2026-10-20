//! Help output snapshots (#203): the top-level and every subcommand's
//! `--help` must match the committed text in `tests/snapshots/help/`, so a
//! change to a default, a limitation or a flag shows up as a reviewed diff.
//!
//! After an intentional help change, regenerate the snapshots with:
//!
//! ```sh
//! UPDATE_HELP_SNAPSHOTS=1 cargo test -p soroban-testkit-cli --test help_snapshots
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

/// Every help screen with a snapshot: (snapshot name, arguments).
const CASES: &[(&str, &[&str])] = &[
    ("soroban-testkit", &["--help"]),
    ("soroban-testkit-short", &["-h"]),
    ("coverage", &["coverage", "--help"]),
    ("coverage-short", &["coverage", "-h"]),
    ("limits", &["limits", "--help"]),
    ("limits-short", &["limits", "-h"]),
    ("audit", &["audit", "--help"]),
    ("audit-short", &["audit", "-h"]),
];

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/help")
        .join(format!("{name}.txt"))
}

/// Runs the binary and returns its help text, normalized so the snapshot
/// is the same on every platform: `\n` line endings, and the binary named
/// `soroban-testkit` (not `soroban-testkit.exe`).
fn help(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_soroban-testkit"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run soroban-testkit");
    assert!(output.status.success(), "{args:?} exited {}", output.status);
    assert!(output.stderr.is_empty(), "{args:?} wrote to stderr");
    String::from_utf8(output.stdout)
        .expect("help is UTF-8")
        .replace("\r\n", "\n")
        .replace("soroban-testkit.exe", "soroban-testkit")
}

#[test]
fn help_output_matches_snapshots() {
    let update = std::env::var_os("UPDATE_HELP_SNAPSHOTS").is_some();
    let mut mismatched = Vec::new();

    for (name, args) in CASES {
        let actual = help(args);
        let path = snapshot_path(name);
        if update {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &actual).unwrap();
            continue;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("missing snapshot {}: {err}", path.display()))
            .replace("\r\n", "\n");
        if actual != expected {
            eprintln!(
                "--- {} (snapshot)\n+++ soroban-testkit {}\n{}",
                path.display(),
                args.join(" "),
                line_diff(&expected, &actual)
            );
            mismatched.push(*name);
        }
    }

    assert!(
        mismatched.is_empty(),
        "help output changed for {mismatched:?}; if intended, rerun with \
         UPDATE_HELP_SNAPSHOTS=1 and commit tests/snapshots/help/"
    );
}

/// A minimal line-by-line diff, enough to point at the changed help text.
fn line_diff(expected: &str, actual: &str) -> String {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let mut diff = String::new();
    for i in 0..expected.len().max(actual.len()) {
        match (expected.get(i), actual.get(i)) {
            (Some(e), Some(a)) if e == a => {}
            (e, a) => {
                if let Some(e) = e {
                    diff.push_str(&format!("-{e}\n"));
                }
                if let Some(a) = a {
                    diff.push_str(&format!("+{a}\n"));
                }
            }
        }
    }
    diff
}

/// The subcommand names listed under `Commands:` in the top-level help.
fn listed_subcommands() -> Vec<String> {
    help(&["--help"])
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .filter(|name| name != "help")
        .collect()
}

#[test]
fn every_listed_subcommand_has_a_help_snapshot() {
    let subcommands = listed_subcommands();
    assert_eq!(subcommands, ["coverage", "limits", "audit"]);
    for name in &subcommands {
        for flag in ["--help", "-h"] {
            assert!(
                CASES.iter().any(|(_, args)| args == &[name.as_str(), flag]),
                "`{name} {flag}` has no snapshot case"
            );
        }
    }
}

#[test]
fn the_internal_probe_command_stays_hidden() {
    assert!(!help(&["--help"]).contains("__limits-probe"));
}

#[test]
fn help_documents_defaults_and_limitations() {
    let top = help(&["--help"]);
    for needle in ["no network access", "EXIT STATUS:", "NO_COLOR"] {
        assert!(
            top.contains(needle),
            "top-level help lacks {needle:?}:\n{top}"
        );
    }

    let coverage = help(&["coverage", "--help"]);
    for needle in [
        "cargo install cargo-llvm-cov",
        "[default: text]",
        "lcov.info",
        "target/llvm-cov/html",
        "Default: no threshold",
    ] {
        assert!(
            coverage.contains(needle),
            "coverage help lacks {needle:?}:\n{coverage}"
        );
    }

    let limits = help(&["limits", "--help"]);
    for needle in [
        "no\nnetwork",
        "16777216",
        "`Vec<Address>`",
        "authorization is mocked",
        "Ledger reads/writes and transaction size are not measured",
    ] {
        let needle_flat = needle.replace('\n', " ");
        assert!(
            limits.contains(needle) || limits.contains(&needle_flat),
            "limits help lacks {needle_flat:?}:\n{limits}"
        );
    }

    let audit = help(&["audit", "--help"]);
    for needle in [
        "not a security product",
        "[default: .]",
        "require_auth",
        "extend_ttl",
        "exit 0 unless --strict",
        "`schema_version` 1",
    ] {
        assert!(
            audit.contains(needle),
            "audit help lacks {needle:?}:\n{audit}"
        );
    }
    assert!(
        !audit.contains("[`"),
        "rustdoc link syntax leaked into help:\n{audit}"
    );
}
