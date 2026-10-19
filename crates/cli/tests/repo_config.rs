//! Regression checks on repository configuration that CI itself depends on:
//! the docs.rs build table, GitHub Actions token scopes, the issue-triage
//! workflow, and the issue-seeding guide. These read files from the
//! workspace only — no network, no YAML/TOML parser dependency — so they
//! match on the exact line shapes the files are written in.

use std::fs;
use std::path::{Path, PathBuf};

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
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

fn workflows() -> Vec<(String, String)> {
    let dir = workspace_root().join(".github/workflows");
    let mut out: Vec<(String, String)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|p| {
            p.extension()
                .is_some_and(|ext| ext == "yml" || ext == "yaml")
        })
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&p).unwrap())
        })
        .collect();
    out.sort();
    out
}

/// Values of every `permissions:` key at any indentation. Matches YAML keys
/// only, so shell text that merely mentions permissions is ignored.
fn permission_keys(yml: &str) -> Vec<&str> {
    yml.lines()
        .filter_map(|l| l.trim_start().strip_prefix("permissions:"))
        .map(str::trim)
        .collect()
}

/// Lines of the top-level `permissions:` mapping (or its inline value).
fn top_level_permissions(yml: &str) -> Vec<String> {
    let mut lines = yml.lines();
    let Some(header) = lines.find(|l| l.starts_with("permissions:")) else {
        return Vec::new();
    };
    let inline = header["permissions:".len()..].trim();
    if !inline.is_empty() {
        return vec![inline.to_string()];
    }
    lines
        .take_while(|l| l.starts_with(' ') || l.is_empty())
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// `(job id, job body)` for every job under the top-level `jobs:` key.
fn jobs(yml: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut in_jobs = false;
    for line in yml.lines() {
        if !line.starts_with(' ') && !line.is_empty() {
            // Only a top-level YAML key ends the `jobs:` block. Embedded
            // script text can also start at column 0 (for example a
            // multi-line shell string), so match on the key shape rather
            // than treating every unindented line as a section boundary.
            if line.trim_end().ends_with(':') {
                in_jobs = line.starts_with("jobs:");
            }
            continue;
        }
        if !in_jobs {
            continue;
        }
        let is_job_header = line.starts_with("  ")
            && !line.starts_with("   ")
            && !line.trim_start().starts_with('#')
            && line.trim_end().ends_with(':');
        if is_job_header {
            out.push((line.trim().trim_end_matches(':').to_string(), String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// Option strings of every `type: dropdown` field in an issue form.
fn dropdown_options(form: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_dropdown = false;
    let mut in_options = false;
    for line in form.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- type:") {
            in_dropdown = trimmed == "- type: dropdown";
            in_options = false;
        } else if in_dropdown && trimmed == "options:" {
            in_options = true;
        } else if in_options {
            match trimmed.strip_prefix("- ") {
                Some(option) => out.push(option.to_string()),
                None => in_options = false,
            }
        }
    }
    out
}

/// `(form option, label)` pairs from the `LABELS` map in issue-triage.yml;
/// `None` for options that intentionally map to no label.
fn triage_label_map(workflow: &str) -> Vec<(String, Option<String>)> {
    workflow
        .lines()
        .skip_while(|l| !l.contains("const LABELS = {"))
        .skip(1)
        .take_while(|l| !l.trim().starts_with("};"))
        .map(|l| {
            let l = l.trim().trim_end_matches(',');
            let (key, value) = l
                .split_once("': ")
                .unwrap_or_else(|| panic!("unexpected LABELS line: {l}"));
            let key = key.trim_start_matches('\'').to_string();
            let value = (value != "null").then(|| value.trim_matches('\'').to_string());
            (key, value)
        })
        .collect()
}

// ---- #294: docs.rs build configuration -------------------------------------

#[test]
fn docs_rs_metadata_builds_host_target_with_docsrs_cfg() {
    let manifest = read("crates/testkit/Cargo.toml");
    let table = manifest
        .split("[package.metadata.docs.rs]")
        .nth(1)
        .expect("crates/testkit/Cargo.toml must declare [package.metadata.docs.rs]");
    let table = table.split("\n[").next().unwrap();

    assert!(
        table.contains(r#"targets = ["x86_64-unknown-linux-gnu"]"#),
        "docs.rs must build only the host target; `testutils` does not compile for wasm:\n{table}"
    );
    assert!(
        table.contains(r#"rustdoc-args = ["--cfg", "docsrs"]"#),
        "docs.rs must pass `--cfg docsrs`:\n{table}"
    );
    assert!(
        manifest.contains(r#"soroban-sdk = { workspace = true, features = ["testutils"] }"#),
        "docs rely on `soroban-sdk/testutils` being enabled unconditionally"
    );
}

#[test]
fn docs_rs_configuration_is_verified_in_ci_and_makefile() {
    let command = "cargo doc -p soroban-testkit --no-deps --target x86_64-unknown-linux-gnu";
    let ci = read(".github/workflows/ci.yml");
    let (_, doc_job) = jobs(&ci)
        .into_iter()
        .find(|(id, _)| id == "doc")
        .expect("ci.yml must have a `doc` job");
    assert!(doc_job.contains(command), "{doc_job}");
    assert!(
        doc_job.contains("RUSTDOCFLAGS: --cfg docsrs -D warnings"),
        "{doc_job}"
    );

    let makefile = read("Makefile");
    assert!(
        makefile.contains(command),
        "Makefile must have a `docsrs` target"
    );
    assert!(makefile.contains("check: ") && makefile.contains(" docsrs "));
}

// ---- #295: CI workflow permissions -----------------------------------------

#[test]
fn every_workflow_declares_top_level_permissions_without_blanket_scopes() {
    let all = workflows();
    assert!(!all.is_empty(), "no workflows found");
    for (name, yml) in all {
        assert!(
            !top_level_permissions(&yml).is_empty(),
            "{name} must declare a top-level `permissions:` block"
        );
        assert!(
            !permission_keys(&yml)
                .iter()
                .any(|v| *v == "write-all" || *v == "read-all"),
            "{name} must list scopes explicitly, not use write-all/read-all"
        );
    }
}

#[test]
fn ci_default_token_is_read_only_and_only_the_scheduled_job_writes() {
    let ci = read(".github/workflows/ci.yml");
    assert_eq!(top_level_permissions(&ci), ["contents: read"]);

    let ci_jobs = jobs(&ci);
    assert!(ci_jobs.len() >= 7, "job parsing broke: {ci_jobs:?}");
    for (id, body) in &ci_jobs {
        if id == "scheduled-sdk-check" || id == "scheduled-nightly-check" {
            assert!(
                body.contains("    permissions:\n      contents: read\n      issues: write\n"),
                "`{id}` must declare its own write scope:\n{body}"
            );
            assert!(
                body.contains("if: github.event_name != 'pull_request'"),
                "`{id}` must never run on pull requests:\n{body}"
            );
        } else {
            assert!(
                permission_keys(body).is_empty(),
                "ci.yml job `{id}` must inherit the read-only default; document any new scope in CONTRIBUTING.md"
            );
        }
    }
}

#[test]
fn contributing_documents_every_job_with_its_own_scope() {
    let contributing = read("CONTRIBUTING.md");
    assert!(contributing.contains("## CI workflow permissions"));
    for (name, yml) in workflows() {
        for (id, body) in jobs(&yml) {
            if !permission_keys(&body).is_empty() {
                assert!(
                    contributing.contains(&format!("`{name}` → `{id}`")),
                    "CONTRIBUTING.md permissions table is missing `{name}` → `{id}`"
                );
            }
        }
    }
}

// ---- #297: issue triage automation -----------------------------------------

#[test]
fn issue_triage_is_bounded_to_opened_issues_and_issue_writes() {
    let wf = read(".github/workflows/issue-triage.yml");

    assert!(
        wf.contains("  issues:\n    types: [opened]\n"),
        "must run only when an issue is opened"
    );
    assert!(
        !wf.contains("pull_request"),
        "must not run on pull requests"
    );
    assert_eq!(top_level_permissions(&wf), ["{}"]);

    let triage_jobs = jobs(&wf);
    assert_eq!(triage_jobs.len(), 1, "{triage_jobs:?}");
    let (_, body) = &triage_jobs[0];
    assert!(
        body.contains("    permissions:\n      issues: write\n"),
        "{body}"
    );

    // The issue body is attacker-controlled: it must never be checked out
    // against, expanded into an expression, or run through a shell.
    assert!(!wf.contains("actions/checkout"));
    assert!(
        !wf.contains("${{"),
        "no expression interpolation in the triage workflow"
    );
    assert!(
        !wf.contains("run:"),
        "no shell steps in the triage workflow"
    );
    assert!(
        wf.contains("Object.hasOwn(LABELS, value)"),
        "only allowlisted values may become labels"
    );
    assert!(!wf.contains("removeLabel"), "triage only adds labels");
}

#[test]
fn every_issue_form_option_maps_to_a_documented_label() {
    let map = triage_label_map(&read(".github/workflows/issue-triage.yml"));
    assert!(!map.is_empty(), "LABELS map not found");
    let spec = read("BUILD_SPEC.md");

    let mut all_options = Vec::new();
    for form in ["bug_report.yml", "feature_request.yml"] {
        let options = dropdown_options(&read(&format!(".github/ISSUE_TEMPLATE/{form}")));
        assert!(!options.is_empty(), "{form} has no dropdowns");
        for option in &options {
            assert!(
                map.iter().any(|(key, _)| key == option),
                "{form} option `{option}` has no entry in issue-triage.yml LABELS"
            );
        }
        all_options.extend(options);
    }

    for (key, label) in &map {
        assert!(
            all_options.contains(key),
            "LABELS entry `{key}` matches no form option"
        );
        if let Some(label) = label {
            assert!(
                spec.contains(&format!("| `{label}` |")),
                "label `{label}` is not in the BUILD_SPEC.md §13 taxonomy"
            );
        }
    }
}

// ---- #252/#253/#255/#250: repository integration configuration ------------

#[test]
fn validation_report_records_versions_and_rerun_policy() {
    let validation = read("VALIDATION.md");
    for required in [
        "**Last run:**",
        "**soroban-testkit:**",
        "**soroban-sdk:** `27.0.6`",
        "**Rust/toolchain:**",
        "at least once every 90 days",
        "pinned SDK",
        "a fresh date alone is not evidence of a rerun",
    ] {
        assert!(
            validation.contains(required),
            "VALIDATION.md must contain `{required}"
        );
    }
}

#[test]
fn issue_config_prompts_authors_to_search_existing_issues() {
    let config = read(".github/ISSUE_TEMPLATE/config.yml");
    assert!(config.contains("contact_links:"));
    assert!(config.contains("Search existing issues before opening a new one"));
    assert!(config.contains("/issues?q=is%3Aissue+sort%3Aupdated-desc"));
}

#[test]
fn label_sync_reconciles_every_documented_module_and_size_label() {
    let workflow = read(".github/workflows/label-sync.yml");
    assert!(workflow.contains("workflow_dispatch: {}"));
    assert!(workflow.contains("permissions: {}"));
    assert!(workflow.contains("issues: write"));
    assert!(workflow.contains("issues.getLabel"));
    assert!(workflow.contains("issues.updateLabel"));
    assert!(workflow.contains("issues.createLabel"));

    for label in [
        "module/core",
        "module/money",
        "module/events",
        "module/auth",
        "module/ttl",
        "module/cli",
        "docs",
        "size/s",
        "size/m",
        "size/l",
    ] {
        assert!(
            workflow.contains(&format!("name: '{label}'")),
            "missing label `{label}`"
        );
    }
}

#[test]
fn license_allowlist_has_a_focused_ci_check() {
    let deny = read("deny.toml");
    let ci = read(".github/workflows/ci.yml");
    let allowlist = deny
        .split("allow = [")
        .nth(1)
        .and_then(|section| section.split(']').next())
        .expect("deny.toml must declare a license allowlist");

    for license in ["Apache-2.0", "MIT", "BSD-2-Clause", "BSD-3-Clause"] {
        assert!(allowlist.contains(&format!("\"{license}\"")));
    }
    assert!(deny.contains("exceptions = []"));
    assert!(ci.contains("run: cargo deny check licenses"));
    assert!(ci.contains("run: cargo deny check"));
}

#[test]
fn issue_form_options_parser_reads_dropdowns_only() {
    let form = "\
body:
  - type: input
    id: a
    attributes:
      label: A
  - type: dropdown
    id: b
    attributes:
      options:
        - one
        - two
    validations:
      required: true
";
    assert_eq!(dropdown_options(form), ["one", "two"]);
}

// ---- #282: supported versions policy ---------------------------------------

#[test]
fn readme_states_a_supported_versions_policy_linked_to_the_toolchain_and_scheduled_check() {
    let readme = read("README.md");
    let section = readme
        .split("## Supported versions")
        .nth(1)
        .expect("README.md must have a `## Supported versions` section")
        .split("\n## ")
        .next()
        .unwrap();

    assert!(
        section.contains("rust-toolchain.toml"),
        "must link the Rust version statement to the pinned toolchain file: {section}"
    );
    assert!(
        section.contains("scheduled-sdk-check"),
        "must link the soroban-sdk version statement to the scheduled compatibility check: {section}"
    );

    // The policy must not drift from what actually governs these versions.
    let toolchain = read("rust-toolchain.toml");
    assert!(toolchain.contains(r#"channel = "stable""#));
    let ci = read(".github/workflows/ci.yml");
    assert!(
        jobs(&ci).iter().any(|(id, _)| id == "scheduled-sdk-check"),
        "ci.yml must still have the scheduled-sdk-check job the README references"
    );
}

// ---- #296: issue seeding guide ---------------------------------------------

#[test]
fn seeding_guide_documents_review_criteria_and_bounded_batches() {
    let spec = read("BUILD_SPEC.md");
    let section = spec
        .split("## 13. Issue seeding for Drips Wave")
        .nth(1)
        .expect("BUILD_SPEC.md §13 missing");
    for required in [
        "### Seeding safeguards",
        "**Readiness review",
        "**Not a duplicate.**",
        "**Bounded batching workflow:**",
        "at most 5 issues",
        "12–15 ready issues, and no more",
    ] {
        assert!(section.contains(required), "§13 must contain `{required}`");
    }
}

// ---- #273: crates.io description review -------------------------------------

#[test]
fn testkit_manifest_description_accurately_summarizes_public_api() {
    let manifest = read("crates/testkit/Cargo.toml");
    let description_line = manifest
        .lines()
        .find(|l| l.starts_with("description = "))
        .expect("crates/testkit/Cargo.toml must have a description");

    for required_capability in [
        "money-math generators",
        "event assertions",
        "ledger time",
        "TTL control",
        "token test doubles",
        "authorization matrices",
    ] {
        assert!(
            description_line.contains(required_capability),
            "crates/testkit/Cargo.toml description must mention `{required_capability}`"
        );
    }
}

// ---- #272: contribution scope examples --------------------------------------

#[test]
fn contributing_documents_accepted_and_rejected_proposal_categories() {
    let contributing = read("CONTRIBUTING.md");
    let scope_section = contributing
        .split("## Scope")
        .nth(1)
        .expect("CONTRIBUTING.md must have a `## Scope` section")
        .split("\n## ")
        .next()
        .unwrap();

    assert!(
        scope_section.contains("### Accepted proposal categories (In Scope)"),
        "CONTRIBUTING.md must document accepted proposal categories"
    );
    assert!(
        scope_section.contains("### Rejected proposal categories (Out of Scope)"),
        "CONTRIBUTING.md must document rejected proposal categories"
    );

    for expected_example in [
        "Isolated test harness",
        "Ledger time and clock control",
        "Money math and invariants",
        "Event assertions and capture",
        "Token test doubles",
        "Authorization matrices",
        "TTL and storage expiry",
        "Mainnet/testnet chain forking",
        "Deployment & lifecycle operations",
        "Frontend & cross-language testing",
        "Network-dependent test utilities",
    ] {
        assert!(
            scope_section.contains(expected_example),
            "Scope section must mention `{expected_example}`"
        );
    }
}

// ---- #271: funding metadata validation --------------------------------------

#[test]
fn funding_metadata_follows_github_schema_with_active_destinations() {
    let funding_json = read("FUNDING.json");
    assert!(
        !funding_json.contains("0x0000000000000000000000000000000000000000"),
        "FUNDING.json must not use placeholder zero address"
    );

    let allowed_keys = [
        "github",
        "patreon",
        "open_collective",
        "ko_fi",
        "tidelift",
        "community_bridge",
        "liberapay",
        "issuehunt",
        "polar",
        "buy_me_a_coffee",
        "custom",
    ];

    assert!(
        allowed_keys
            .iter()
            .any(|key| funding_json.contains(&format!("\"{key}\""))),
        "FUNDING.json must declare supported GitHub schema keys"
    );

    let funding_yml = read(".github/FUNDING.yml");
    assert!(
        allowed_keys
            .iter()
            .any(|key| funding_yml.contains(&format!("{key}:"))),
        ".github/FUNDING.yml must declare supported GitHub schema keys"
    );
}

// ---- #270: smoke test install docs ------------------------------------------

#[test]
fn readme_documents_clean_environment_install_validation() {
    let readme = read("README.md");
    let cli_section = readme
        .split("## CLI")
        .nth(1)
        .expect("README.md must have a `## CLI` section")
        .split("\n## ")
        .next()
        .unwrap();

    assert!(
        cli_section.contains("### Installation"),
        "CLI section must have installation instructions"
    );
    assert!(
        cli_section.contains("### Clean environment validation"),
        "CLI section must document clean environment validation"
    );
    assert!(
        cli_section.contains("CARGO_INSTALL_ROOT"),
        "Installation docs must demonstrate isolated CARGO_INSTALL_ROOT"
    );

    let script_path = workspace_root().join("scripts/smoke_test_cli_install.sh");
    assert!(
        script_path.exists(),
        "scripts/smoke_test_cli_install.sh must exist"
    );

    let makefile = read("Makefile");
    assert!(
        makefile.contains("smoke-test-install"),
        "Makefile must have smoke-test-install target"
    );
}

// ---- #262-#265: README scope docs ---------------------------------------------

/// The README's `## Example` code block, which is also a doctest.
fn readme_example() -> String {
    read("README.md")
        .split("## Example")
        .nth(1)
        .expect("README.md must have a `## Example` section")
        .split("```")
        .nth(1)
        .expect("`## Example` must contain a code block")
        .to_string()
}

#[test]
fn readme_limits_output_lists_exactly_the_fields_the_cli_measures() {
    let readme = read("README.md");
    let limits_src = read("crates/cli/src/commands/limits.rs");
    let sample = readme
        .split("A run prints the last successful ramp value")
        .nth(1)
        .expect("README.md must show sample `limits` output")
        .split("```text")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();

    let fields: Vec<&str> = sample
        .lines()
        .filter_map(|l| l.strip_prefix("  ")?.split_once(": ").map(|(k, _)| k))
        .filter(|k| !k.starts_with('('))
        .collect();
    assert_eq!(fields, ["instructions", "memory bytes"], "{sample}");
    for field in fields {
        assert!(
            limits_src.contains(&format!("println!(\"  {field}: {{")),
            "limits.rs must print the `{field}` field the README shows"
        );
    }
}

#[test]
fn readme_event_example_warns_about_capture_scope_before_the_capture() {
    let example = readme_example();
    let warning = example
        .find("Capture scope:")
        .expect("example must explain event capture scope");
    let capture = example.find("env.events_during(").unwrap();
    let first_later_call = example.find("token.assert_balance(").unwrap();
    assert!(warning < capture, "warning must precede events_during");
    assert!(capture < first_later_call);
}

#[test]
fn readme_example_pairs_whole_unit_and_base_unit_mints() {
    let example = readme_example();
    assert!(example.contains("token.mint(&buyer, 100);"), "{example}");
    assert!(
        example.contains("token.mint_raw(&other_buyer, 1_000_000_000);"),
        "{example}"
    );
    assert!(example.contains("whole tokens"), "{example}");
    assert!(example.contains("base units"), "{example}");
}

#[test]
fn readme_audit_section_lists_every_rule_and_disclaims_guarantees() {
    let readme = read("README.md");
    let audit_src = read("crates/cli/src/commands/audit.rs");
    let section = readme
        .split("### Audit command")
        .nth(1)
        .expect("README.md must have an `### Audit command` section")
        .split("\n## ")
        .next()
        .unwrap();

    for rule in [
        "missing-require-auth",
        "unchecked-i128-arithmetic",
        "missing-ttl-bump",
    ] {
        assert!(
            section.contains(&format!("`{rule}`")),
            "README must list {rule}"
        );
        assert!(
            audit_src.contains(&format!("\"{rule}\"")),
            "audit.rs lost {rule}"
        );
    }
    assert!(section.contains("not a security guarantee"), "{section}");
}

// ---- #254: module-level examples -------------------------------------------

#[test]
fn every_testkit_module_documents_a_setup_operation_assertion_example() {
    for module in ["auth", "core", "events", "ledger", "money", "tokens", "ttl"] {
        let src = read(&format!("crates/testkit/src/{module}/mod.rs"));
        assert!(
            src.contains("//! # Example\n"),
            "crates/testkit/src/{module}/mod.rs must have a module-level `# Example`"
        );
        assert!(
            src.contains("//! ```\n"),
            "crates/testkit/src/{module}/mod.rs example must be a runnable doctest"
        );
    }
}

// ---- #256: SDK upgrade checklist -------------------------------------------

#[test]
fn sdk_upgrade_checklist_covers_each_testkit_surface_and_the_scheduled_job() {
    let contributing = read("CONTRIBUTING.md");
    let checklist = contributing
        .split("## Maintainer SDK review checklist")
        .nth(1)
        .expect("CONTRIBUTING.md must have a `## Maintainer SDK review checklist` section")
        .split("\n## ")
        .next()
        .unwrap();

    assert!(checklist.contains("scheduled-sdk-check"), "{checklist}");
    for area in ["testutils", "ledger", "event", "auth", "TTL"] {
        assert!(
            checklist.contains(area),
            "checklist must re-verify `{area}`:\n{checklist}"
        );
    }
}

// ---- #257: CLI package and binary names ------------------------------------

#[test]
fn cli_package_and_binary_names_agree_across_manifest_docs_and_smoke_test() {
    let manifest = read("crates/cli/Cargo.toml");
    assert!(
        manifest.contains("name = \"soroban-testkit-cli\"\n"),
        "package name must be `soroban-testkit-cli`"
    );
    assert!(
        manifest.contains("[[bin]]\nname = \"soroban-testkit\"\n"),
        "installed binary must be `soroban-testkit`"
    );

    let readme = read("README.md");
    assert!(readme.contains("cargo install soroban-testkit-cli --locked"));
    assert!(readme.contains("(installs as `soroban-testkit`)"));

    let smoke = read("scripts/smoke_test_cli_install.sh");
    assert!(
        smoke.contains("$TMP_INSTALL_ROOT/bin/soroban-testkit\""),
        "smoke test must check the installed `soroban-testkit` binary"
    );
}

// ---- #332: workspace-inherited manifest keys -------------------------------

#[test]
fn every_manifest_inherited_key_is_defined_in_workspace_package() {
    let root = read("Cargo.toml");
    let defined: Vec<&str> = root
        .split("[workspace.package]")
        .nth(1)
        .expect("Cargo.toml must declare [workspace.package]")
        .split("\n[")
        .next()
        .unwrap()
        .lines()
        .filter_map(|l| l.split_once(" = ").map(|(key, _)| key.trim()))
        .collect();

    for manifest in [
        "crates/cli/Cargo.toml",
        "crates/testkit/Cargo.toml",
        "examples/vault/Cargo.toml",
    ] {
        let mut in_package = false;
        for line in read(manifest).lines() {
            if line.starts_with('[') {
                in_package = line == "[package]";
                continue;
            }
            if !in_package {
                continue;
            }
            if let Some(key) = line.strip_suffix(".workspace = true") {
                assert!(
                    defined.contains(&key),
                    "{manifest} inherits `{key}` from [workspace.package], which does not define it"
                );
            }
        }
    }
}

// ---- #225: license and repository metadata consistency ---------------------

/// Every `key = value` in `[workspace.package]`.
fn workspace_package_field(key: &str) -> String {
    let root = read("Cargo.toml");
    let table = root
        .split("[workspace.package]")
        .nth(1)
        .expect("Cargo.toml must declare [workspace.package]")
        .split("\n[")
        .next()
        .unwrap();
    table
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key} = ")))
        .unwrap_or_else(|| panic!("[workspace.package] must define `{key}`"))
        .trim()
        .trim_matches('"')
        .to_string()
}

#[test]
fn every_member_inherits_the_workspace_license_and_repository() {
    assert_eq!(workspace_package_field("license"), "Apache-2.0");
    assert_eq!(
        workspace_package_field("repository"),
        "https://github.com/soroban-testkit/testkit-blockchain"
    );

    for manifest in [
        "crates/cli/Cargo.toml",
        "crates/testkit/Cargo.toml",
        "examples/vault/Cargo.toml",
    ] {
        let contents = read(manifest);
        for key in ["license", "repository"] {
            assert!(
                contents.contains(&format!("{key}.workspace = true")),
                "{manifest} must inherit `{key}` from [workspace.package]"
            );
            assert!(
                !contents.contains(&format!("\n{key} = ")),
                "{manifest} must not redefine `{key}`; inherit it instead"
            );
        }
    }
}

#[test]
fn documentation_never_claims_a_dual_mit_license() {
    for doc in ["README.md", "ARCHITECTURE.md", "examples/vault/README.md"] {
        let contents = read(doc);
        assert!(
            !contents.contains("MIT OR Apache"),
            "{doc} must state the workspace license (Apache-2.0), not a dual MIT/Apache license"
        );
    }
    assert!(
        read("examples/vault/README.md").contains("Apache-2.0"),
        "examples/vault/README.md must name the workspace license"
    );
    assert_eq!(workspace_package_field("license"), "Apache-2.0");
}

// ---- #226: workspace-wide rustfmt scope ------------------------------------

#[test]
fn format_checks_cover_the_whole_workspace_everywhere() {
    let command = "cargo fmt --all --check";

    let ci = read(".github/workflows/ci.yml");
    let (_, lint_job) = jobs(&ci)
        .into_iter()
        .find(|(id, _)| id == "lint")
        .expect("ci.yml must have a `lint` job");
    assert!(
        lint_job.contains(command),
        "CI must format-check every workspace member and example:\n{lint_job}"
    );

    let makefile = read("Makefile");
    assert!(
        makefile.contains(&format!("\t{command}")),
        "the `fmt` target must use `{command}`"
    );

    let contributing = read("CONTRIBUTING.md");
    assert!(
        contributing.contains("cargo fmt --all --check"),
        "CONTRIBUTING.md must document the workspace-wide format command"
    );
}

// ---- #227: workspace feature boundaries ------------------------------------

#[test]
fn testutils_stays_confined_to_dev_and_host_side_contexts() {
    let workspace = read("Cargo.toml");
    assert!(
        workspace.contains("members = [\"crates/*\", \"examples/vault\"]"),
        "examples must stay workspace members so their features are resolved together"
    );

    // The library and CLI are host-side test tooling: enabling `testutils`
    // in their normal dependencies is the point.
    for host_crate in ["crates/testkit/Cargo.toml", "crates/cli/Cargo.toml"] {
        assert!(
            read(host_crate)
                .contains(r#"soroban-sdk = { workspace = true, features = ["testutils"] }"#),
            "{host_crate} must enable `soroban-sdk/testutils` unconditionally"
        );
    }

    // The example contract is wasm-buildable: `testutils` must never leak
    // into its normal dependency graph, only into dev/feature contexts.
    let vault = read("examples/vault/Cargo.toml");
    let normal_deps = vault
        .split("[dependencies]")
        .nth(1)
        .and_then(|s| s.split("\n[").next())
        .expect("examples/vault/Cargo.toml must have a [dependencies] table");
    assert!(
        normal_deps.contains("soroban-sdk.workspace = true"),
        "vault's normal soroban-sdk dependency must not enable `testutils`:\n{normal_deps}"
    );
    assert!(
        !normal_deps.contains("testutils"),
        "vault's normal soroban-sdk dependency must not enable `testutils`:\n{normal_deps}"
    );
    assert!(
        vault.contains(r#"soroban-sdk = { workspace = true, features = ["testutils"] }"#),
        "vault must enable `testutils` only under [dev-dependencies]"
    );

    // CLI-only parsing/argument crates must not creep into the library.
    let testkit = read("crates/testkit/Cargo.toml");
    let testkit_deps = testkit
        .split("[dependencies]")
        .nth(1)
        .and_then(|s| s.split("\n[").next())
        .expect("crates/testkit/Cargo.toml must have a [dependencies] table");
    for cli_only in [
        "syn",
        "quote",
        "proc-macro2",
        "walkdir",
        "clap",
        "soroban-spec",
    ] {
        assert!(
            !testkit_deps
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{cli_only} "))),
            "`{cli_only}` is CLI-only and must not appear in crates/testkit's normal dependencies"
        );
    }
}

#[test]
fn ci_enforces_the_feature_boundary_with_a_cargo_tree_policy_check() {
    let ci = read(".github/workflows/ci.yml");
    let (_, lint_job) = jobs(&ci)
        .into_iter()
        .find(|(id, _)| id == "lint")
        .expect("ci.yml must have a `lint` job");

    assert!(
        lint_job.contains("cargo tree -p vault -e normal -e features"),
        "CI must check that `testutils` does not leak into the example's normal graph:\n{lint_job}"
    );
    assert!(
        lint_job.contains("soroban-sdk feature \"testutils\""),
        "the vault feature check must look for `soroban-sdk feature \"testutils\"`"
    );
    assert!(
        lint_job.contains("cargo tree -p soroban-testkit -e normal"),
        "CI must keep the library/CLI dependency-boundary check"
    );
}
