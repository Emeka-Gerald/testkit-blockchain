use soroban_sdk::Address;

use crate::core::{TestEnv, TestkitError};

type InvokeFn<'a> = Box<dyn Fn(&Address) -> Result<(), soroban_sdk::Error> + 'a>;

struct EntryPointSpec<'a> {
    name: String,
    allowed: Vec<Address>,
    invoke: InvokeFn<'a>,
}

/// A declarative table of who may call what, so that a contract's full
/// caller/entry-point grid is checked by default instead of the four cases
/// someone thought to write by hand.
///
/// Register each privileged entry point with [`AuthMatrix::entry_point`],
/// then call [`AuthMatrix::assert_enforced`] to check that every allowed
/// address succeeds and every other address known to the matrix is
/// rejected, for every entry point.
pub struct AuthMatrix<'a> {
    // Ties the matrix's lifetime to its TestEnv; entry point closures
    // capture their own references into it independently.
    #[allow(dead_code)]
    env: &'a TestEnv,
    entry_points: Vec<EntryPointSpec<'a>>,
}

impl<'a> AuthMatrix<'a> {
    /// Start an empty matrix against `env`.
    pub fn new(env: &'a TestEnv) -> Self {
        Self {
            env,
            entry_points: Vec::new(),
        }
    }

    /// Register a privileged entry point: its name (for failure messages),
    /// the addresses permitted to call it, and a closure that attempts the
    /// call as a given caller, returning `Ok(())` if it succeeded and
    /// `Err` if the contract rejected it.
    ///
    /// `invoke` is typically built from a generated client's `try_*`
    /// method with the candidate address mocked as the caller — see
    /// [`AuthMatrix::assert_enforced`]'s example.
    pub fn entry_point(
        mut self,
        name: &str,
        allowed: &[Address],
        invoke: impl Fn(&Address) -> Result<(), soroban_sdk::Error> + 'a,
    ) -> Self {
        self.entry_points.push(EntryPointSpec {
            name: name.to_string(),
            allowed: allowed.to_vec(),
            invoke: Box::new(invoke),
        });
        self
    }

    /// For every registered entry point, assert every allowed address
    /// succeeds and every other address known to the matrix (i.e. named as
    /// `allowed` on *some* entry point) fails.
    ///
    /// # Panics
    ///
    /// Panics with a [`TestkitError::AssertionFailed`] naming every
    /// violating (entry point, caller) cell, followed by the full grid
    /// (every cell's expected vs. actual outcome) so a passing cell isn't
    /// mistaken for a gap in coverage.
    ///
    /// # Example
    ///
    /// ```
    /// use soroban_testkit::auth::AuthMatrix;
    /// use soroban_testkit::core::TestEnv;
    ///
    /// let env = TestEnv::new();
    /// let owner = env.address();
    ///
    /// // A single-entry-point matrix where the only allowed caller is
    /// // itself: trivially enforced.
    /// AuthMatrix::new(&env)
    ///     .entry_point("noop", &[owner.clone()], move |caller| {
    ///         if *caller == owner {
    ///             Ok(())
    ///         } else {
    ///             Err(soroban_sdk::Error::from_contract_error(1))
    ///         }
    ///     })
    ///     .assert_enforced();
    /// ```
    pub fn assert_enforced(self) {
        let mut known: Vec<Address> = Vec::new();
        for ep in &self.entry_points {
            for addr in &ep.allowed {
                if !known.contains(addr) {
                    known.push(addr.clone());
                }
            }
        }

        let mut violations = Vec::new();
        let mut grid = String::new();

        for ep in &self.entry_points {
            grid.push_str(&format!("  [{}]\n", ep.name));
            for addr in &known {
                let expected = ep.allowed.contains(addr);
                let actual = (ep.invoke)(addr).is_ok();
                grid.push_str(&format!(
                    "    {addr:?}: expected={} actual={}{}\n",
                    if expected { "pass" } else { "reject" },
                    if actual { "pass" } else { "reject" },
                    if expected == actual {
                        ""
                    } else {
                        "  <-- MISMATCH"
                    },
                ));
                if expected != actual {
                    violations.push(format!(
                        "entry point {:?}: caller {addr:?} expected to {} but {}",
                        ep.name,
                        if expected { "succeed" } else { "be rejected" },
                        if actual { "succeeded" } else { "was rejected" },
                    ));
                }
            }
        }

        if !violations.is_empty() {
            panic!(
                "{}",
                TestkitError::AssertionFailed(format!(
                    "auth matrix violated ({} of {} cells):\n{}\nfull grid:\n{grid}",
                    violations.len(),
                    self.entry_points.len() * known.len(),
                    violations.join("\n"),
                ))
            );
        }
    }
}

impl TestEnv {
    /// Assert that `f` fails when it runs without the authorization it
    /// requires — the single-call counterpart to [`AuthMatrix`], for a
    /// caller that just needs "this must reject an unauthorized attempt"
    /// without a full grid.
    ///
    /// # Panics
    ///
    /// Panics with a [`TestkitError::AssertionFailed`] if `f` completes
    /// (returns) instead of failing.
    ///
    /// # Example
    ///
    /// ```should_panic
    /// use soroban_testkit::core::TestEnv;
    ///
    /// let env = TestEnv::new();
    /// // A closure that succeeds unconditionally can never satisfy this.
    /// env.assert_requires_auth(|| {});
    /// ```
    pub fn assert_requires_auth(&self, f: impl FnOnce()) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        if result.is_ok() {
            panic!(
                "{}",
                TestkitError::AssertionFailed(
                    "expected the call to fail without authorization, but it succeeded".into()
                )
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::{MockAuth, MockAuthInvoke};
    use soroban_sdk::IntoVal;
    use vault::{VaultClient, VaultError};

    fn flatten<T>(
        result: Result<
            Result<T, soroban_sdk::ConversionError>,
            Result<VaultError, soroban_sdk::InvokeError>,
        >,
    ) -> Result<(), soroban_sdk::Error> {
        match result {
            Ok(Ok(_)) => Ok(()),
            _ => Err(soroban_sdk::Error::from_contract_error(0)),
        }
    }

    #[test]
    fn assert_enforced_catches_a_real_missing_auth_bug() {
        let env = TestEnv::new();
        let owner = env.address();
        let attacker = env.address();
        let vault_id = env.env().register(vault::Vault, ());
        VaultClient::new(env.env(), &vault_id).initialize(&owner);
        VaultClient::new(env.env(), &vault_id).deposit(&100);

        // `deposit` is legitimately open to anyone, which is what puts
        // `attacker` into the matrix's "known addresses" set. `withdraw`
        // (the buggy entry point) never calls owner.require_auth(), so
        // when we check that only `owner` may withdraw, `attacker` — a
        // known address the matrix now cross-checks against every entry
        // point — incorrectly succeeds too. That's exactly the bug.
        let matrix = AuthMatrix::new(&env)
            .entry_point(
                "deposit",
                &[owner.clone(), attacker.clone()],
                |caller: &Address| {
                    let result = VaultClient::new(env.env(), &vault_id)
                        .mock_auths(&[MockAuth {
                            address: caller,
                            invoke: &MockAuthInvoke {
                                contract: &vault_id,
                                fn_name: "deposit",
                                args: (1_i128,).into_val(env.env()),
                                sub_invokes: &[],
                            },
                        }])
                        .try_deposit(&1);
                    flatten(result)
                },
            )
            .entry_point(
                "withdraw",
                std::slice::from_ref(&owner),
                |caller: &Address| {
                    let result = VaultClient::new(env.env(), &vault_id)
                        .mock_auths(&[MockAuth {
                            address: caller,
                            invoke: &MockAuthInvoke {
                                contract: &vault_id,
                                fn_name: "withdraw",
                                args: (10_i128,).into_val(env.env()),
                                sub_invokes: &[],
                            },
                        }])
                        .try_withdraw(&10);
                    flatten(result)
                },
            );

        let message = crate::testing::assert_panics_with_code("TESTKIT_ASSERTION_FAILED", || {
            matrix.assert_enforced();
        });
        assert!(
            message.contains("withdraw"),
            "failure message should name the entry point: {message}"
        );
        assert!(
            message.contains(&format!("{attacker:?}")),
            "failure message should name the offending caller: {message}"
        );
    }

    #[test]
    fn assert_enforced_passes_once_the_bug_is_fixed() {
        let env = TestEnv::new();
        let owner = env.address();
        let other = env.address();
        let vault_id = env.env().register(vault::Vault, ());
        VaultClient::new(env.env(), &vault_id).initialize(&owner);
        VaultClient::new(env.env(), &vault_id).deposit(&100);

        // Same shape as the buggy-case test (two entry points, so `other`
        // is a known address cross-checked against withdraw_checked), but
        // against the fixed entry point: `other` is correctly rejected.
        AuthMatrix::new(&env)
            .entry_point(
                "deposit",
                &[owner.clone(), other.clone()],
                |caller: &Address| {
                    let result = VaultClient::new(env.env(), &vault_id)
                        .mock_auths(&[MockAuth {
                            address: caller,
                            invoke: &MockAuthInvoke {
                                contract: &vault_id,
                                fn_name: "deposit",
                                args: (1_i128,).into_val(env.env()),
                                sub_invokes: &[],
                            },
                        }])
                        .try_deposit(&1);
                    flatten(result)
                },
            )
            .entry_point(
                "withdraw_checked",
                std::slice::from_ref(&owner),
                |caller: &Address| {
                    let result = VaultClient::new(env.env(), &vault_id)
                        .mock_auths(&[MockAuth {
                            address: caller,
                            invoke: &MockAuthInvoke {
                                contract: &vault_id,
                                fn_name: "withdraw_checked",
                                args: (10_i128,).into_val(env.env()),
                                sub_invokes: &[],
                            },
                        }])
                        .try_withdraw_checked(&10);
                    flatten(result)
                },
            )
            .assert_enforced();
    }

    #[test]
    fn assert_requires_auth_passes_when_call_panics() {
        let env = TestEnv::new();
        env.assert_requires_auth(|| panic!("simulated auth failure"));
    }

    #[test]
    #[should_panic(expected = "expected the call to fail without authorization")]
    fn assert_requires_auth_panics_when_call_succeeds() {
        let env = TestEnv::new();
        env.assert_requires_auth(|| {});
    }
}
