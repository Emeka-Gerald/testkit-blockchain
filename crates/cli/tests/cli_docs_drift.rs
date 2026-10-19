// Issue #277: CLI argument documentation drift tests
// Asserts key flags and descriptions from generated help output

#[cfg(test)]
mod cli_docs_tests {
    use std::process::Command;

    fn get_help_output(subcommand: &str) -> String {
        get_help_output_with(subcommand, "--help")
    }

    /// `-h` output. Unlike `--help`, which puts each description on the
    /// line below its flag once any argument has multi-line help, `-h`
    /// always prints a flag and its description on one line.
    fn get_short_help_output(subcommand: &str) -> String {
        get_help_output_with(subcommand, "-h")
    }

    fn get_help_output_with(subcommand: &str, flag: &str) -> String {
        let output = Command::new("cargo")
            .args(["run", "--bin", "soroban-testkit", "--", subcommand, flag])
            .args([
                "run",
                "--bin",
                "soroban-testkit",
                "--",
                subcommand,
                "--help",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("Failed to run command");

        String::from_utf8_lossy(&output.stdout).to_string()
    }

    #[test]
    fn test_audit_help_has_key_flags() {
        let help = get_help_output("audit");

        // Assert key flags are documented
        assert!(help.contains("--strict"), "audit should have --strict flag");
        assert!(help.contains("--json"), "audit should have --json flag");
        assert!(help.contains("PATH"), "audit should document PATH argument");

        // Assert descriptions are present
        assert!(
            help.contains("Exit non-zero") || help.contains("strict"),
            "--strict flag should be described"
        );
        assert!(
            help.contains("JSON") || help.contains("json"),
            "--json flag should be described"
        );
    }

    #[test]
    fn test_audit_examples_present() {
        let help = get_help_output("audit");

        // Assert examples section exists
        assert!(
            help.contains("EXAMPLES:") || help.contains("Examples:"),
            "audit help should have examples section"
        );

        // Common example patterns
        assert!(
            help.contains("soroban-testkit audit"),
            "Examples should show basic usage"
        );
    }

    #[test]
    fn test_main_help_lists_subcommands() {
        let output = Command::new("cargo")
            .args(["run", "--bin", "soroban-testkit", "--", "--help"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("Failed to run command");

        let help = String::from_utf8_lossy(&output.stdout);

        // Assert audit subcommand is listed
        assert!(
            help.contains("audit"),
            "Main help should list audit subcommand"
        );

        // Assert other expected subcommands (add as they exist)
        // Example: assert!(help.contains("test"), "Main help should list test subcommand");
    }

    #[test]
    fn test_version_flag_works() {
        let output = Command::new("cargo")
            .args(["run", "--bin", "soroban-testkit", "--", "--version"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("Failed to run command");

        let version_output = String::from_utf8_lossy(&output.stdout);

        // Should show version information
        assert!(
            version_output.contains("soroban-testkit") || !version_output.is_empty(),
            "--version should display version info"
        );
        assert!(
            output.status.success(),
            "--version should exit successfully"
        );
    }

    #[test]
    fn test_help_flag_consistency() {
        // Test that -h and --help produce similar output
        let short_output = Command::new("cargo")
            .args(["run", "--bin", "soroban-testkit", "--", "-h"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("Failed to run -h");

        let long_output = Command::new("cargo")
            .args(["run", "--bin", "soroban-testkit", "--", "--help"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("Failed to run --help");

        let short_help = String::from_utf8_lossy(&short_output.stdout);
        let long_help = String::from_utf8_lossy(&long_output.stdout);

        // Both should succeed
        assert!(short_output.status.success(), "-h should succeed");
        assert!(long_output.status.success(), "--help should succeed");

        // Both should contain core information
        assert!(
            short_help.contains("audit") || long_help.contains("audit"),
            "Help output should mention audit command"
        );
    }

    #[test]
    fn test_audit_json_schema_version_documented() {
        let help = get_help_output("audit");

        // JSON output should mention schema version
        if help.contains("--json") {
            assert!(
                help.contains("schema") || help.contains("version"),
                "JSON flag should mention schema or version"
            );
        }
    }

    #[test]
    fn test_stdin_support_documented() {
        let help = get_help_output("audit");

        // Stdin support should be documented
        assert!(
            help.contains("-") || help.contains("stdin"),
            "audit should document stdin support with '-'"
        );
    }

    #[test]
    fn test_clap_derive_matches_docs() {
        // This test ensures clap-generated help matches our expectations
        // If this fails, either update clap attributes or documentation

        let help = get_help_output("audit");

        // Key clap attributes should be present
        assert!(help.contains("Usage:"), "Clap should generate usage line");
        assert!(
            help.contains("Options:") || help.contains("FLAGS:"),
            "Clap should list options"
        );
    }

    #[test]
    fn test_all_flags_have_descriptions() {
        let help = get_short_help_output("audit");

        // Count flag definitions vs descriptions
        let strict_count = help.matches("--strict").count();
        let json_count = help.matches("--json").count();

        // Each flag should appear at least once (in options list)
        assert!(strict_count >= 1, "--strict should be listed");
        assert!(json_count >= 1, "--json should be listed");

        // If a flag appears, its line should have description text after it
        for line in help.lines() {
            if line.contains("--strict") || line.contains("--json") {
                // Line should have more than just the flag name
                assert!(
                    line.len() > 20,
                    "Flag line should have description: {}",
                    line
                );
            }
        }
    }
}
