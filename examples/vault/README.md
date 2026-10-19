# Vault Example Fixture

## License and Usage

This example contract is licensed under the same terms as the soroban-testkit workspace (see [LICENSE](../../LICENSE) in the repository root).

**This is a test fixture, not production code.**

## Purpose

The `vault` example is a deliberately buggy Soroban contract used in:
- soroban-testkit's documentation
- Integration tests for the CLI
- Examples of testing patterns and error detection

## Key Characteristics

- **Intentionally Buggy**: Contains known issues for testing detection tooling
- **Not For Production**: Never use this contract in production environments
- **Test Fixture Only**: Marked with `publish = false` to prevent crates.io publication
- **Documentation Example**: Demonstrates common contract patterns and testing approaches

## Metadata

- **Package**: `vault`
- **Purpose**: Testing and documentation fixture
- **Status**: Intentionally buggy, maintained for testing purposes only
- **License**: Apache-2.0, same as the workspace
