## Summary

<!-- What does this change, and why? -->

## Module

<!-- Which module (per BUILD_SPEC.md §4) does this touch? -->

## Checklist

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] Documentation is updated or not needed
- [ ] User-visible behavior changes are described and tested, or marked N/A
- [ ] Public API changes have doc comments with a runnable `# Example`
- [ ] New assertion helpers have both a passing and a
      `#[should_panic(expected = "...")]` test
- [ ] Commit messages follow Conventional Commits

## Spec discrepancies

<!-- If current soroban-sdk / stellar CLI docs contradicted BUILD_SPEC.md,
     note the discrepancy here so the spec can be corrected. -->
