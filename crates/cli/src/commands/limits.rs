use std::panic;
use std::process::Command;

use clap::Args;
use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::xdr::{ScSpecEntry, ScSpecFunctionInputV0, ScSpecFunctionV0, ScSpecTypeDef};
use soroban_sdk::{Address, Env, IntoVal, Symbol, Val, Vec as SVec};

use super::output::Output;
use super::CliError;

/// Arguments for `soroban-testkit limits`.
#[derive(Args)]
#[command(after_help = "EXAMPLES:\n    \
    soroban-testkit limits --contract target/wasm32v1-none/release/my_contract.wasm --fn batch_payout --ramp recipients\n    \
    soroban-testkit limits --contract my_contract.wasm --fn transfer --ramp amount")]
pub struct LimitsArgs {
    /// Path to the compiled contract .wasm file
    /// (e.g. `target/wasm32v1-none/release/<crate>.wasm`)
    #[arg(long, value_name = "PATH", verbatim_doc_comment)]
    contract: std::path::PathBuf,
    /// The contract function to ramp, as named in the contract's spec.
    #[arg(long = "fn", value_name = "NAME")]
    function: String,
    /// The parameter of --fn to increase on each attempt: an integer type
    /// (ramped as the value) or `Vec<Address>` (ramped as its length)
    #[arg(long, value_name = "PARAM", verbatim_doc_comment)]
    ramp: String,
}

/// Hidden: runs exactly one probe (a single ramp value) and reports its
/// outcome via exit code. See [`run`]'s doc comment for why this exists
/// as a separate process rather than an in-process call.
#[derive(Args)]
pub struct ProbeArgs {
    #[arg(long)]
    contract: std::path::PathBuf,
    #[arg(long = "fn")]
    function: String,
    #[arg(long)]
    ramp: String,
    #[arg(long)]
    value: u32,
}

/// Empirically discovers a resource ceiling by invoking `--fn` with an
/// increasing `--ramp` parameter — natively, with no network access —
/// until it fails, then reports the last successful value plus the CPU
/// instructions and memory consumed at that point.
///
/// # Why this spawns a subprocess per attempt
///
/// Ramping deliberately drives a real, compiled `.wasm` contract past its
/// resource limits. Verified empirically: when that failure happens
/// *during actual WASM execution* (as opposed to a native, non-WASM
/// `#[contract]` registration), the host can abort the whole process
/// (`thread caused non-unwinding panic. aborting.`) rather than return an
/// error or unwind — neither `catch_unwind` nor `try_invoke_contract`
/// (the SDK's own no-panic call path) prevents this for the WASM target.
/// So each attempt runs in its own child process (`soroban-testkit
/// __limits-probe`, hidden from `--help`): if that child aborts, only the
/// child dies, and the exit code alone tells this command whether that
/// ramp value succeeded.
///
/// # Scope
///
/// The ramp parameter must be a numeric type (`u32`/`i32`/`u64`/`i64`/
/// `u128`/`i128`, ramped as the value itself) or `Vec<Address>` (ramped as
/// the number of generated addresses) — this covers the common "maximum
/// recipients in a batch operation" question directly. Every other
/// parameter is filled with a fixed default (an address, `0`, an empty
/// collection, ...). A parameter type this command doesn't know how to
/// default (`Map`, `Tuple`, `Option`, `Result`, a user-defined type, ...)
/// is reported as an error rather than guessed at.
///
/// Ledger read/write counts and transaction size are **not** reported:
/// they come from a transaction's simulated resource footprint, which
/// this command's native invocation does not produce (that requires the
/// network-dependent `stellar contract invoke` simulation path, which
/// this crate deliberately avoids — see its "zero network access"
/// constraint). Only CPU instructions and memory, available locally via
/// the host's budget, are reported.
pub fn run(args: LimitsArgs, out: Output) -> Result<(), CliError> {
    // Validate the configuration (function exists, ramp parameter exists,
    // every parameter's type is one this command knows how to default or
    // ramp) up front, in-process, before spawning any probes — this is
    // just type/spec checking, not invocation, so it carries none of the
    // abort risk documented above, and it means a typo'd --fn or --ramp
    // reports its actual cause instead of the generic "failed at value 1"
    // a swallowed child-process error would otherwise produce.
    let (_, env, function, ramp_index) = load(&args.contract, &args.function, &args.ramp)?;
    build_args(&env, &function, ramp_index, 1)?;
    drop(env);

    let self_exe = std::env::current_exe()
        .map_err(|err| CliError(format!("failed to locate this binary: {err}")))?;

    let probe = |value: u32| -> Result<bool, CliError> {
        let status = Command::new(&self_exe)
            .arg("__limits-probe")
            .arg("--contract")
            .arg(&args.contract)
            .arg("--fn")
            .arg(&args.function)
            .arg("--ramp")
            .arg(&args.ramp)
            .arg("--value")
            .arg(value.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|err| CliError(format!("failed to spawn probe: {err}")))?;
        out.detail(format!(
            "probe {}={value}: {}",
            args.ramp,
            if status.success() { "ok" } else { "failed" }
        ));
        Ok(status.success())
    };

    // Find a failing upper bound by doubling.
    let mut last_ok: Option<u32> = None;
    let mut low = 1u32;
    let mut high = None;
    loop {
        if probe(low)? {
            last_ok = Some(low);
            match low.checked_mul(2) {
                Some(next) => low = next,
                None => {
                    high = None;
                    break;
                }
            }
        } else {
            high = Some(low);
            break;
        }
        if low > 1 << 24 {
            break;
        }
    }

    let Some(last_ok) = last_ok else {
        return Err(CliError(format!(
            "{:?} failed even at the smallest ramp value (1) for parameter {:?}",
            args.function, args.ramp
        )));
    };

    // Binary search between the last success and the first failure for a
    // tighter bound, if we found one.
    let mut best = last_ok;
    if let Some(hi_start) = high {
        let mut lo = last_ok;
        let mut hi = hi_start;
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            if probe(mid)? {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        best = lo;
    }

    // Measure resources at the best-known-good value, in-process this
    // time (a known-successful call is safe to make directly).
    let (instructions, memory_bytes) = measure(&args, best)?;

    println!(
        "last successful {} for {:?}: {best}",
        args.ramp, args.function
    );
    println!("  instructions: {instructions}");
    println!("  memory bytes: {memory_bytes}");
    out.note(
        "  (ledger reads/writes and transaction size are not measured by this command; \
         see --help)",
    );

    Ok(())
}

/// The hidden probe entry point: performs exactly one invocation and
/// returns `Ok(())` (exit 0) or `Err` (exit 1) — no measurement, no
/// output. Run in its own process by [`run`].
pub fn run_probe(args: ProbeArgs) -> Result<(), CliError> {
    let _quiet = QuietPanics::install();
    let (contract_id, env, function, ramp_index) =
        load(&args.contract, &args.function, &args.ramp)?;
    let args_vec = build_args(&env, &function, ramp_index, args.value)?;
    let func = Symbol::new(&env, &args.function);
    if invoke_succeeds(&env, &contract_id, &func, args_vec) {
        Ok(())
    } else {
        Err(CliError("probe failed".to_string()))
    }
}

fn measure(args: &LimitsArgs, value: u32) -> Result<(u64, u64), CliError> {
    let (contract_id, env, function, ramp_index) =
        load(&args.contract, &args.function, &args.ramp)?;
    let func = Symbol::new(&env, &args.function);
    let mut budget = env.cost_estimate().budget();
    budget.reset_default();
    let args_vec = build_args(&env, &function, ramp_index, value)?;
    if !invoke_succeeds(&env, &contract_id, &func, args_vec) {
        return Err(CliError(
            "internal error: the ramp value chosen as successful failed on re-measurement"
                .to_string(),
        ));
    }
    Ok((budget.cpu_instruction_cost(), budget.memory_bytes_cost()))
}

/// Silences the default panic hook for the duration it's held, restoring
/// the previous hook on drop, so a probe's failure doesn't dump a panic
/// message and the host's diagnostic event log to stderr.
type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send>;

struct QuietPanics {
    previous: Option<PanicHook>,
}

impl QuietPanics {
    fn install() -> Self {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        Self {
            previous: Some(previous),
        }
    }
}

impl Drop for QuietPanics {
    fn drop(&mut self) {
        if let Some(hook) = self.previous.take() {
            panic::set_hook(hook);
        }
    }
}

fn load(
    contract: &std::path::Path,
    function: &str,
    ramp: &str,
) -> Result<(Address, Env, ScSpecFunctionV0, usize), CliError> {
    let wasm = std::fs::read(contract)
        .map_err(|err| CliError(format!("failed to read {}: {err}", contract.display())))?;

    let entries = soroban_spec::read::from_wasm(&wasm)
        .map_err(|err| CliError(format!("failed to read contract spec: {err}")))?;

    let func_spec = find_function_spec(entries, contract, function)?;
    let ramp_index = ramp_index_of(&func_spec, function, ramp)?;

    let env = Env::new_with_config(soroban_sdk::testutils::EnvTestConfig {
        capture_snapshot_at_drop: false,
    });
    // Auth is not what this command measures — it exists to find a
    // resource ceiling, not to prove who may call the function — and each
    // probe runs in its own throwaway process/Env, so blanket-approving
    // auth here carries none of the AuthMatrix-correctness risk that a
    // shared, longer-lived TestEnv would (see TestToken's mint, which
    // deliberately does NOT do this for that reason).
    env.mock_all_auths_allowing_non_root_auth();
    let contract_id = env.register(wasm.as_slice(), ());

    Ok((contract_id, env, func_spec, ramp_index))
}

/// Picks `function` out of a contract's spec entries. Split out of [`load`]
/// so the lookup can be tested against hand-built spec fixtures, without a
/// compiled `.wasm`.
fn find_function_spec(
    entries: Vec<ScSpecEntry>,
    contract: &std::path::Path,
    function: &str,
) -> Result<ScSpecFunctionV0, CliError> {
    entries
        .into_iter()
        .find_map(|entry| match entry {
            ScSpecEntry::FunctionV0(f) if f.name.to_string() == function => Some(f),
            _ => None,
        })
        .ok_or_else(|| {
            CliError(format!(
                "no function named {function:?} in {}'s spec",
                contract.display()
            ))
        })
}

/// Returns the position of the `--ramp` parameter in `func_spec`'s inputs.
fn ramp_index_of(
    func_spec: &ScSpecFunctionV0,
    function: &str,
    ramp: &str,
) -> Result<usize, CliError> {
    func_spec
        .inputs
        .iter()
        .position(|input| input.name.to_utf8_string_lossy() == ramp)
        .ok_or_else(|| CliError(format!("{function:?} has no parameter named {ramp:?}")))
}

fn build_args(
    env: &Env,
    function: &ScSpecFunctionV0,
    ramp_index: usize,
    ramp_value: u32,
) -> Result<SVec<Val>, CliError> {
    let admin = Address::generate(env);
    let mut vals = SVec::new(env);
    for (i, input) in function.inputs.iter().enumerate() {
        let val = if i == ramp_index {
            ramp_val(env, &input.type_, ramp_value)?
        } else {
            default_val(env, &admin, &input.type_, input)?
        };
        vals.push_back(val);
    }
    Ok(vals)
}

fn invoke_succeeds(env: &Env, contract_id: &Address, func: &Symbol, args: SVec<Val>) -> bool {
    // try_invoke_contract is the SDK's own no-panic call path, used here
    // so a probe that succeeds doesn't need catch_unwind at all — only a
    // *failing* real-WASM call risks the process abort documented above,
    // and that only ever happens in the isolated child process.
    matches!(
        env.try_invoke_contract::<Val, soroban_sdk::Error>(contract_id, func, args),
        Ok(Ok(_))
    )
}

fn ramp_val(env: &Env, type_: &ScSpecTypeDef, ramp_value: u32) -> Result<Val, CliError> {
    match type_ {
        ScSpecTypeDef::U32 => Ok(ramp_value.into_val(env)),
        ScSpecTypeDef::I32 => Ok((ramp_value as i32).into_val(env)),
        ScSpecTypeDef::U64 => Ok((ramp_value as u64).into_val(env)),
        ScSpecTypeDef::I64 => Ok((ramp_value as i64).into_val(env)),
        ScSpecTypeDef::U128 => Ok((ramp_value as u128).into_val(env)),
        ScSpecTypeDef::I128 => Ok((ramp_value as i128).into_val(env)),
        ScSpecTypeDef::Vec(inner) if matches!(*inner.element_type, ScSpecTypeDef::Address) => {
            let mut addrs = SVec::new(env);
            for _ in 0..ramp_value {
                addrs.push_back(Address::generate(env));
            }
            Ok(addrs.into_val(env))
        }
        other => Err(CliError(format!(
            "the ramp parameter's type ({other:?}) isn't supported yet; supported ramp types \
             are u32/i32/u64/i64/u128/i128 and Vec<Address>"
        ))),
    }
}

fn default_val(
    env: &Env,
    admin: &Address,
    type_: &ScSpecTypeDef,
    input: &ScSpecFunctionInputV0,
) -> Result<Val, CliError> {
    match type_ {
        // A parameter literally named `token` is, by overwhelming Soroban
        // convention, a token contract address — and a batch/payout-style
        // function will call transfer() on it and needs `admin` to hold a
        // real balance, not just a syntactically valid address. Deploying
        // a funded Stellar Asset Contract for this one case is a
        // name-based heuristic, not a semantic guarantee, but it's what
        // makes `limits` usable against a real payout contract at all
        // (verified against sororail-contracts' batch_payout, whose
        // execute_equal(funder, token, recipients, amount_each) is
        // exactly this shape).
        ScSpecTypeDef::Address if input.name.to_utf8_string_lossy() == "token" => {
            let issuer = Address::generate(env);
            let sac = env.register_stellar_asset_contract_v2(issuer.clone());
            let token_address = sac.address();
            let amount = i128::MAX / 2;
            StellarAssetClient::new(env, &token_address)
                .mock_auths(&[MockAuth {
                    address: &issuer,
                    invoke: &MockAuthInvoke {
                        contract: &token_address,
                        fn_name: "mint",
                        args: (admin.clone(), amount).into_val(env),
                        sub_invokes: &[],
                    },
                }])
                .mint(admin, &amount);
            Ok(token_address.into_val(env))
        }
        ScSpecTypeDef::Address => Ok(admin.into_val(env)),
        ScSpecTypeDef::Bool => Ok(false.into_val(env)),
        // 1, not 0: verified against sororail-contracts' batch_payout,
        // whose execute_equal(..., amount_each: i128) requires a positive
        // amount and rejects 0 with its own InvalidAmount error before
        // any resource-limit-relevant work happens. There's no default
        // that's safe for every contract's validation rules, but 1 is
        // the smaller mistake: far more contracts reject a non-positive
        // amount than reject exactly 1.
        ScSpecTypeDef::U32 => Ok(1u32.into_val(env)),
        ScSpecTypeDef::I32 => Ok(1i32.into_val(env)),
        ScSpecTypeDef::U64 => Ok(1u64.into_val(env)),
        ScSpecTypeDef::I64 => Ok(1i64.into_val(env)),
        ScSpecTypeDef::U128 => Ok(1u128.into_val(env)),
        ScSpecTypeDef::I128 => Ok(1i128.into_val(env)),
        ScSpecTypeDef::Symbol => Ok(Symbol::new(env, "x").into_val(env)),
        ScSpecTypeDef::String => Ok(soroban_sdk::String::from_str(env, "").into_val(env)),
        ScSpecTypeDef::Bytes => Ok(soroban_sdk::Bytes::new(env).into_val(env)),
        ScSpecTypeDef::Vec(_) => Ok(SVec::<Val>::new(env).into_val(env)),
        other => Err(CliError(format!(
            "parameter {:?} has type {other:?}, which this command doesn't know how to \
             default yet; only the --ramp parameter needs a type this command understands \
             today, every other parameter needs a supported default type",
            input.name.to_utf8_string_lossy()
        ))),
    }
}

/// Spec fixtures built by hand from the XDR types, so the parsing and
/// ramp/default logic is tested for every supported `ScSpecTypeDef` without
/// building a `.wasm` first.
#[cfg(test)]
mod tests {
    use soroban_sdk::xdr::{ScSpecTypeMap, ScSpecTypeVec, ScSymbol};

    use super::*;

    fn vec_of(element: ScSpecTypeDef) -> ScSpecTypeDef {
        ScSpecTypeDef::Vec(Box::new(ScSpecTypeVec {
            element_type: Box::new(element),
        }))
    }

    fn input(name: &str, type_: ScSpecTypeDef) -> ScSpecFunctionInputV0 {
        ScSpecFunctionInputV0 {
            doc: "".try_into().expect("empty doc fits"),
            name: name.try_into().expect("fixture name fits in 30 bytes"),
            type_,
        }
    }

    fn function(name: &str, inputs: Vec<ScSpecFunctionInputV0>) -> ScSpecFunctionV0 {
        ScSpecFunctionV0 {
            doc: "".try_into().expect("empty doc fits"),
            name: ScSymbol(name.try_into().expect("fixture name fits in 32 bytes")),
            inputs: inputs.try_into().expect("fixture has at most 10 inputs"),
            outputs: Vec::<ScSpecTypeDef>::new().try_into().expect("no outputs"),
        }
    }

    /// Every type `--ramp` accepts. Keep in sync with [`ramp_val`].
    fn ramp_types() -> Vec<ScSpecTypeDef> {
        vec![
            ScSpecTypeDef::U32,
            ScSpecTypeDef::I32,
            ScSpecTypeDef::U64,
            ScSpecTypeDef::I64,
            ScSpecTypeDef::U128,
            ScSpecTypeDef::I128,
            vec_of(ScSpecTypeDef::Address),
        ]
    }

    /// Every type a non-ramp parameter can default to. Keep in sync with
    /// [`default_val`].
    fn default_types() -> Vec<ScSpecTypeDef> {
        vec![
            ScSpecTypeDef::Address,
            ScSpecTypeDef::Bool,
            ScSpecTypeDef::U32,
            ScSpecTypeDef::I32,
            ScSpecTypeDef::U64,
            ScSpecTypeDef::I64,
            ScSpecTypeDef::U128,
            ScSpecTypeDef::I128,
            ScSpecTypeDef::Symbol,
            ScSpecTypeDef::String,
            ScSpecTypeDef::Bytes,
            vec_of(ScSpecTypeDef::U32),
        ]
    }

    #[test]
    fn finds_the_named_function_among_other_entries() {
        let entries = vec![
            ScSpecEntry::FunctionV0(function("deposit", vec![])),
            ScSpecEntry::FunctionV0(function("batch_payout", vec![])),
        ];
        let found = find_function_spec(entries, std::path::Path::new("vault.wasm"), "batch_payout")
            .expect("batch_payout is in the spec");
        assert_eq!(found.name.to_string(), "batch_payout");
    }

    #[test]
    fn reports_a_missing_function_with_the_contract_path() {
        let err = find_function_spec(
            vec![ScSpecEntry::FunctionV0(function("deposit", vec![]))],
            std::path::Path::new("vault.wasm"),
            "withdraw",
        )
        .expect_err("withdraw is not in the spec");
        assert!(err.0.contains("no function named \"withdraw\""), "{err}");
        assert!(err.0.contains("vault.wasm"), "{err}");
    }

    #[test]
    fn finds_the_ramp_parameter_position() {
        let func = function(
            "batch_payout",
            vec![
                input("funder", ScSpecTypeDef::Address),
                input("recipients", vec_of(ScSpecTypeDef::Address)),
            ],
        );
        assert_eq!(
            ramp_index_of(&func, "batch_payout", "recipients").expect("recipients is an input"),
            1
        );
    }

    #[test]
    fn reports_a_missing_ramp_parameter() {
        let func = function(
            "batch_payout",
            vec![input("funder", ScSpecTypeDef::Address)],
        );
        let err = ramp_index_of(&func, "batch_payout", "recipients").expect_err("no such input");
        assert!(
            err.0.contains("has no parameter named \"recipients\""),
            "{err}"
        );
    }

    #[test]
    fn every_supported_ramp_type_builds_a_value() {
        let env = Env::default();
        for ty in ramp_types() {
            assert!(
                ramp_val(&env, &ty, 3).is_ok(),
                "ramp type {ty:?} should be supported"
            );
        }
    }

    #[test]
    fn unsupported_ramp_types_are_rejected_with_an_actionable_message() {
        let env = Env::default();
        for ty in [
            ScSpecTypeDef::Bool,
            ScSpecTypeDef::Symbol,
            ScSpecTypeDef::String,
        ] {
            let err = ramp_val(&env, &ty, 3).expect_err("not a ramp type");
            assert!(err.0.contains("isn't supported yet"), "{err}");
            assert!(err.0.contains("u32/i32/u64/i64/u128/i128"), "{err}");
        }
    }

    #[test]
    fn every_supported_default_type_builds_a_value() {
        let env = Env::default();
        let admin = soroban_sdk::Address::generate(&env);
        for ty in default_types() {
            let fixture = input("param", ty.clone());
            assert!(
                default_val(&env, &admin, &ty, &fixture).is_ok(),
                "default type {ty:?} should be supported"
            );
        }
    }

    #[test]
    fn a_parameter_named_token_gets_a_funded_token_contract() {
        let env = Env::default();
        let admin = soroban_sdk::Address::generate(&env);
        let fixture = input("token", ScSpecTypeDef::Address);
        assert!(default_val(&env, &admin, &ScSpecTypeDef::Address, &fixture).is_ok());
    }

    #[test]
    fn unknown_default_types_are_rejected_with_an_actionable_message() {
        let env = Env::default();
        let admin = soroban_sdk::Address::generate(&env);
        let ty = ScSpecTypeDef::Map(Box::new(ScSpecTypeMap {
            key_type: Box::new(ScSpecTypeDef::Symbol),
            value_type: Box::new(ScSpecTypeDef::U32),
        }));
        let fixture = input("balances", ty.clone());
        let err = default_val(&env, &admin, &ty, &fixture).expect_err("Map has no default");
        assert!(err.0.contains("doesn't know how to default"), "{err}");
    }
}
