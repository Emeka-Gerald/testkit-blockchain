// Issue #275: Rust source parse robustness tests
// Tests audit command handles edition syntax and Rust constructs

#[cfg(test)]
mod parse_robustness_tests {
    use std::process::Command;

    fn audit_source(source: &str) -> Result<String, String> {
        // Run audit command with source from stdin
        let output = Command::new("cargo")
            .args(["run", "--bin", "soroban-testkit", "--", "audit", "-"])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(source.as_bytes())?;
                }
                child.wait_with_output()
            })
            .map_err(|e| format!("Failed to run audit: {}", e))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    #[test]
    fn test_parse_rust_2021_async_fn() {
        // Rust 2021 edition: async fn syntax
        let source = r#"
use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub async fn async_method(env: Env, addr: Address) -> u32 {
        addr.require_auth();
        42
    }
}
"#;

        // Should parse without error (async not supported in Soroban but parser should handle)
        let result = audit_source(source);
        // Parser should not crash, may report findings or parse error
        assert!(result.is_ok() || result.unwrap_err().contains("parse"));
    }

    #[test]
    fn test_parse_const_generic() {
        // Rust 2021: const generics
        let source = r#"
use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn generic_method<const N: usize>(env: Env, addr: Address) -> [u32; N] {
        addr.require_auth();
        [0; N]
    }
}
"#;

        let result = audit_source(source);
        assert!(result.is_ok() || result.unwrap_err().contains("parse"));
    }

    #[test]
    fn test_parse_try_blocks() {
        // Rust 2021: try blocks (unstable)
        let source = r#"
use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn try_method(env: Env, addr: Address) -> Result<u32, ()> {
        addr.require_auth();
        let result: Result<u32, ()> = try {
            42
        };
        result
    }
}
"#;

        let result = audit_source(source);
        // May fail to parse try blocks (unstable), but should give actionable error
        if let Err(e) = result {
            assert!(
                e.contains("parse") || e.contains("unexpected") || e.contains("syntax"),
                "Error should be actionable: {}",
                e
            );
        }
    }

    #[test]
    fn test_parse_let_else() {
        // Rust 2021 (1.65+): let-else syntax
        let source = r#"
use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn let_else_method(env: Env, addr: Address, value: Option<u32>) -> u32 {
        addr.require_auth();
        let Some(val) = value else {
            return 0;
        };
        val
    }
}
"#;

        let result = audit_source(source);
        assert!(result.is_ok(), "Should parse let-else syntax");
    }

    #[test]
    fn test_parse_complex_macro() {
        // Complex macro invocations
        let source = r#"
use soroban_sdk::{contract, contractimpl, Address, Env};

macro_rules! define_method {
    ($name:ident) => {
        pub fn $name(env: Env, addr: Address) -> u32 {
            addr.require_auth();
            42
        }
    };
}

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    define_method!(generated_method);
}
"#;

        let result = audit_source(source);
        // Macros expand before parsing, should work
        assert!(result.is_ok() || result.unwrap_err().contains("macro"));
    }

    #[test]
    fn test_incompatibility_message_actionable() {
        // Test that parse errors have actionable messages
        let invalid_source = "pub fn {{{{{";

        let result = audit_source(invalid_source);

        if let Err(e) = result {
            // Error message should mention parsing or syntax
            assert!(
                e.contains("parse")
                    || e.contains("syntax")
                    || e.contains("expected")
                    || e.contains("unexpected"),
                "Parse error should be actionable: {}",
                e
            );
        }
    }

    #[test]
    fn test_parse_attributes() {
        // Test parsing of common attributes
        let source = r#"
use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    #[allow(dead_code)]
    #[inline(always)]
    #[cfg(feature = "test")]
    pub fn attributed_method(env: Env, addr: Address) -> u32 {
        addr.require_auth();
        42
    }
}
"#;

        let result = audit_source(source);
        assert!(result.is_ok(), "Should parse attributes");
    }

    #[test]
    fn test_parse_lifetime_annotations() {
        // Test lifetime annotations
        let source = r#"
use soroban_sdk::{contract, contractimpl, Address, Env};

#[contract]
pub struct Contract;

#[contractimpl]
impl Contract {
    pub fn lifetime_method<'a>(env: Env, addr: Address, data: &'a [u8]) -> &'a [u8] {
        addr.require_auth();
        data
    }
}
"#;

        let result = audit_source(source);
        assert!(result.is_ok() || result.unwrap_err().contains("lifetime"));
    }
}
