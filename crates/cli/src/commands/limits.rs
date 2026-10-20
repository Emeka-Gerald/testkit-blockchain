use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::process::Command;

use clap::Args;
use soroban_sdk::testutils::{Address as _, MockAuth, MockAuthInvoke};
use soroban_sdk::token::StellarAssetClient;
use soroban_sdk::xdr::{ScSpecEntry, ScSpecFunctionInputV0, ScSpecFunctionV0, ScSpecTypeDef};
use soroban_sdk::{Address, Env, IntoVal, Symbol, Val, Vec as SVec};

use super::output::Output;
use super::{quote, CliError};

/// The signal handler only sets an atomic flag; no IO runs inside it.
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
mod signals {
    use super::{CANCELLED, Ordering};
    use std::os::raw::c_int;
    const SIGINT: c_int = 2;
    const SIGTERM: c_int = 15;
    unsafe extern "C" {
        fn signal(signum: c_int, handler: usize) -> usize;
    }
    extern "C" fn cancel(_: c_int) {
        CANCELLED.store(true, Ordering::Relaxed);
    }
    pub struct SignalGuard {
        previous_int: usize,
        previous_term: usize,
    }
    impl SignalGuard {
        pub fn install() -> Self {
            CANCELLED.store(false, Ordering::Relaxed);
            // SAFETY: The installed handler only writes an atomic flag.
            let previous_int = unsafe { signal(SIGINT, cancel as usize) };
            let previous_term = unsafe { signal(SIGTERM, cancel as usize) };
            Self { previous_int, previous_term }
        }
    }
    impl Drop for SignalGuard {
        fn drop(&mut self) {
            // SAFETY: Restore the handlers returned by signal().
            unsafe {
                signal(SIGINT, self.previous_int);
                signal(SIGTERM, self.previous_term);
            }
        }
    }
}

#[cfg(not(unix))]
mod signals {
    pub struct SignalGuard;
    impl SignalGuard {
        pub fn install() -> Self { Self }
    }
}

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
    // Results are read back in ramp order, so the boundary matches a sequential
    // run. Probes past the first failure are discarded; see BUILD_SPEC.md.
    /// Probe up to N ramp values at once (1 to 8, default 1).
    #[arg(long, value_name = "N", default_value_t = 1)]
    jobs: usize,
    /// Generate and print one probe's arguments without invoking the contract.
    /// This validates the contract spec and is useful when diagnosing defaults.
    #[arg(long)]
    dry_run: bool,
}

/// Largest accepted `--jobs`. Each concurrent probe is a full child process.
pub const MAX_JOBS: usize = 8;

/// Largest ramp value tried while doubling. Values above it are never probed.
const SEARCH_CEILING: u32 = 1 << 24;

/// Outcome of one search phase. `Cancelled` carries the best known success.
#[derive(Debug, PartialEq)]
enum Search<T> {
    Done(T),
    Cancelled { last_ok: Option<u32> },
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
    if args.jobs == 0 || args.jobs > MAX_JOBS {
        return Err(CliError(format!(
            "--jobs must be between 1 and {MAX_JOBS} (got {})",
            args.jobs
        )));
    }
    let _signal_guard = signals::SignalGuard::install();
    // Validate the configuration (function exists, ramp parameter exists,
    // every parameter's type is one this command knows how to default or
    // ramp) up front, in-process, before spawning any probes — this is
    // just type/spec checking, not invocation, so it carries none of the
    // abort risk documented above, and it means a typo'd --fn or --ramp
    // reports its actual cause instead of the generic "failed at value 1"
    // a swallowed child-process error would otherwise produce.
    let (_, env, function, ramp_index) = load(&args.contract, &args.function, &args.ramp)?;
    let preview = build_args(&env, &function, ramp_index, 1)?;
    if args.dry_run {
        println!("probe {}=1 generated arguments: {preview:?}", args.ramp);
        return Ok(());
    }
    drop(env);

    let self_exe = std::env::current_exe()
        .map_err(|err| CliError(format!("failed to locate this binary: {err}")))?;

    // Runs one batch of probes concurrently, then logs them in ramp order.
    // Returns `None` when the run is cancelled before the batch starts.
    let mut probe_batch = |values: &[u32]| -> Result<Option<Vec<bool>>, CliError> {
        if CANCELLED.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let results =
            probe_concurrently(values, |value| run_probe_process(&self_exe, &args, value));
        let mut outcomes = Vec::with_capacity(values.len());
        for (value, result) in values.iter().zip(results) {
            let ok = result?;
            out.detail(format!(
                "probe {}={value}: {}",
                args.ramp,
                if ok { "ok" } else { "failed" }
            ));
            outcomes.push(ok);
        }
        Ok(Some(outcomes))
    };

    // Find a failing upper bound by doubling.
    let (ladder_ok, ladder_failed) = match find_ladder_bounds(args.jobs, &mut probe_batch)? {
        Search::Done(bounds) => bounds,
        Search::Cancelled { last_ok } => return report_interrupted(&args, last_ok),
    };

    if CANCELLED.load(Ordering::Relaxed) {
        return report_interrupted(&args, ladder_ok);
    }

    let Some(last_ok) = ladder_ok else {
        return Err(CliError(format!(
            "{} failed even at the smallest ramp value (1) for parameter {}; check that \
             the function succeeds with default arguments (run with --verbose to see each probe)",
            quote(&args.function),
            quote(&args.ramp)
        )));
    };

    // Binary search between the last success and the first failure for a
    // tighter bound, if we found one.
    let mut best = last_ok;
    if let Some(hi_start) = ladder_failed {
        best = match refine_boundary(last_ok, hi_start, args.jobs, &mut probe_batch)? {
            Search::Done(boundary) => boundary,
            Search::Cancelled { last_ok } => return report_interrupted(&args, last_ok),
        };
    }

    if CANCELLED.load(Ordering::Relaxed) {
        return report_interrupted(&args, Some(best));
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

/// Runs one probe in its own process. `Ok(true)` means the ramp value succeeded.
fn run_probe_process(
    self_exe: &std::path::Path,
    args: &LimitsArgs,
    value: u32,
) -> Result<bool, CliError> {
    let status = Command::new(self_exe)
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
    Ok(status.success())
}

/// Runs `work` on each value on its own thread and returns the results in
/// the order of `values`. A worker that panics reports an error for its value.
fn probe_concurrently(
    values: &[u32],
    work: impl Fn(u32) -> Result<bool, CliError> + Sync,
) -> Vec<Result<bool, CliError>> {
    std::thread::scope(|scope| {
        let work = &work;
        let workers: Vec<_> = values
            .iter()
            .map(|&value| scope.spawn(move || work(value)))
            .collect();
        workers
            .into_iter()
            .map(|worker| {
                worker.join().unwrap_or_else(|_| {
                    Err(CliError(
                        "an isolated probe worker panicked; re-run with --jobs 1".to_string(),
                    ))
                })
            })
            .collect()
    })
}

/// The doubling ladder value after `value`, or `None` past [`SEARCH_CEILING`].
fn ladder_successor(value: u32) -> Option<u32> {
    value.checked_mul(2).filter(|next| *next <= SEARCH_CEILING)
}

/// Up to `jobs` consecutive ladder values starting at `start`, never past
/// [`SEARCH_CEILING`]. A `jobs` of 0 is treated as 1.
fn ladder_batch(start: u32, jobs: usize) -> Vec<u32> {
    let mut batch = Vec::new();
    let mut current = Some(start);
    while let Some(value) = current {
        if batch.len() == jobs.max(1) {
            break;
        }
        batch.push(value);
        current = ladder_successor(value);
    }
    batch
}

/// Doubling phase. Probes ladder values in batches of `jobs`, reads each
/// batch in order, and stops at the first failure. Returns the last success
/// and the first failure. Both are `None` when the search reached the ceiling
/// without failing.
fn find_ladder_bounds<P>(
    jobs: usize,
    mut probe_batch: P,
) -> Result<Search<(Option<u32>, Option<u32>)>, CliError>
where
    P: FnMut(&[u32]) -> Result<Option<Vec<bool>>, CliError>,
{
    let mut last_ok = None;
    let mut next = Some(1u32);
    while let Some(start) = next {
        let batch = ladder_batch(start, jobs);
        let Some(results) = probe_batch(&batch)? else {
            return Ok(Search::Cancelled { last_ok });
        };
        for (value, ok) in batch.into_iter().zip(results) {
            if !ok {
                return Ok(Search::Done((last_ok, Some(value))));
            }
            last_ok = Some(value);
            next = ladder_successor(value);
        }
    }
    Ok(Search::Done((last_ok, None)))
}

/// Refinement phase. `lo` is a known success and `hi` a known failure.
/// Each round probes up to `jobs` evenly spaced points strictly between
/// them, then keeps the sub-interval that straddles the first failure. With
/// `jobs` = 1 this is the classic midpoint binary search. The search ends
/// when `lo` and `hi` are adjacent, and returns `lo`.
fn refine_boundary<P>(
    mut lo: u32,
    mut hi: u32,
    jobs: usize,
    mut probe_batch: P,
) -> Result<Search<u32>, CliError>
where
    P: FnMut(&[u32]) -> Result<Option<Vec<bool>>, CliError>,
{
    while lo + 1 < hi {
        let gap = hi - lo;
        // At least one point, and never more points than fit strictly inside.
        let count = jobs.max(1).min((gap - 1) as usize);
        let points: Vec<u32> = (1..=count as u32)
            .map(|i| lo + gap * i / (count as u32 + 1))
            .collect();
        let Some(results) = probe_batch(&points)? else {
            return Ok(Search::Cancelled { last_ok: Some(lo) });
        };
        match results.iter().position(|ok| !ok) {
            Some(0) => hi = points[0],
            Some(first_failure) => {
                lo = points[first_failure - 1];
                hi = points[first_failure];
            }
            None => lo = points[count - 1],
        }
    }
    Ok(Search::Done(lo))
}

/// A cancelled run reports its last successful probe without claiming
/// a proven maximum or attempting a fresh WASM invocation.
fn report_interrupted(args: &LimitsArgs, last_ok: Option<u32>) -> Result<(), CliError> {
    match last_ok {
        Some(value) => {
            println!("interrupted; last successful {} for {:?}: {value} (partial result, not a proven maximum)", args.ramp, args.function);
            println!("  resource measurements unavailable after cancellation");
            Ok(())
        }
        None => Err(CliError(format!(
            "limits search interrupted before any successful probe for {:?}", args.function
        ))),
    }
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
    let wasm = std::fs::read(contract).map_err(|err| {
        CliError(format!(
            "failed to read {}: {err}; pass the path to a compiled contract .wasm",
            quote(contract.display())
        ))
    })?;

    let entries = soroban_spec::read::from_wasm(&wasm).map_err(|err| spec_error(contract, &err))?;

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

/// An actionable message for a `.wasm` whose contract spec can't be read.
/// `FromWasmError`'s own `Display` is just "reading wasm" etc., which
/// names neither the file nor the fix.
fn spec_error(contract: &std::path::Path, err: &soroban_spec::read::FromWasmError) -> CliError {
    use soroban_spec::read::FromWasmError;

    let path = quote(contract.display());
    CliError(match err {
        FromWasmError::Read(source) => format!(
            "{path} is not a valid WebAssembly module ({source}); pass a contract built for \
             wasm32v1-none, e.g. target/wasm32v1-none/release/<crate>.wasm"
        ),
        FromWasmError::Parse(source) => format!(
            "{path} has a malformed contract spec ({source}); rebuild it with a current \
             soroban-sdk"
        ),
        FromWasmError::NotFound => format!(
            "{path} has no contract spec (no `contractspecv0` section); is it a Soroban \
             contract built with soroban-sdk?"
        ),
    })
}

/// A Rust-like name for a contract spec type, for error messages, e.g.
/// `Vec<Address>`, `Map<Symbol, u32>` or `Option<BytesN<32>>`, in place
/// of the XDR struct's `Debug` dump.
fn type_name(type_: &ScSpecTypeDef) -> String {
    let simple = match type_ {
        ScSpecTypeDef::Val => "Val",
        ScSpecTypeDef::Bool => "bool",
        ScSpecTypeDef::Void => "()",
        ScSpecTypeDef::Error => "Error",
        ScSpecTypeDef::U32 => "u32",
        ScSpecTypeDef::I32 => "i32",
        ScSpecTypeDef::U64 => "u64",
        ScSpecTypeDef::I64 => "i64",
        ScSpecTypeDef::Timepoint => "Timepoint",
        ScSpecTypeDef::Duration => "Duration",
        ScSpecTypeDef::U128 => "u128",
        ScSpecTypeDef::I128 => "i128",
        ScSpecTypeDef::U256 => "U256",
        ScSpecTypeDef::I256 => "I256",
        ScSpecTypeDef::Bytes => "Bytes",
        ScSpecTypeDef::String => "String",
        ScSpecTypeDef::Symbol => "Symbol",
        ScSpecTypeDef::Address => "Address",
        ScSpecTypeDef::MuxedAddress => "MuxedAddress",
        ScSpecTypeDef::Option(option) => {
            return format!("Option<{}>", type_name(&option.value_type))
        }
        ScSpecTypeDef::Result(result) => {
            return format!(
                "Result<{}, {}>",
                type_name(&result.ok_type),
                type_name(&result.error_type)
            )
        }
        ScSpecTypeDef::Vec(vec) => return format!("Vec<{}>", type_name(&vec.element_type)),
        ScSpecTypeDef::Map(map) => {
            return format!(
                "Map<{}, {}>",
                type_name(&map.key_type),
                type_name(&map.value_type)
            )
        }
        ScSpecTypeDef::Tuple(tuple) => {
            let items: Vec<String> = tuple.value_types.iter().map(type_name).collect();
            return format!("({})", items.join(", "));
        }
        ScSpecTypeDef::BytesN(bytes) => return format!("BytesN<{}>", bytes.n),
        ScSpecTypeDef::Udt(udt) => return udt.name.to_utf8_string_lossy(),
    };
    simple.to_string()
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
                "no function named {} in {}'s spec; check the --fn name",
                quote(function),
                quote(contract.display())
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
        .ok_or_else(|| {
            let params: Vec<String> = func_spec
                .inputs
                .iter()
                .map(|input| quote(input.name.to_utf8_string_lossy()))
                .collect();
            let params = if params.is_empty() {
                "it takes no parameters".to_string()
            } else {
                format!("its parameters are {}", params.join(", "))
            };
            CliError(format!(
                "{} has no parameter named {}; {params}",
                quote(function),
                quote(ramp)
            ))
        })
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
            "the ramp parameter's type ({}) isn't supported yet; supported ramp types \
             are u32/i32/u64/i64/u128/i128 and Vec<Address>",
            quote(type_name(other))
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
            "parameter {} has type {}, which this command doesn't know how to \
             default yet; every parameter other than --ramp needs one of Address, bool, \
             u32/i32/u64/i64/u128/i128, Symbol, String, Bytes or Vec",
            quote(input.name.to_utf8_string_lossy()),
            quote(type_name(other))
        ))),
    }
}

/// Spec fixtures built by hand from the XDR types, so the parsing and
/// ramp/default logic is tested for every supported `ScSpecTypeDef` without
/// building a `.wasm` first.
#[cfg(test)]
mod tests {
    use soroban_sdk::xdr::{
        ScSpecTypeBytesN, ScSpecTypeMap, ScSpecTypeOption, ScSpecTypeResult, ScSpecTypeTuple,
        ScSpecTypeUdt, ScSpecTypeVec, ScSymbol,
    };

    use super::*;

    /// A probe where a value succeeds exactly when it is at most `threshold`.
    fn threshold_probe(
        threshold: u32,
    ) -> impl FnMut(&[u32]) -> Result<Option<Vec<bool>>, CliError> {
        move |values: &[u32]| -> Result<Option<Vec<bool>>, CliError> {
            Ok(Some(values.iter().map(|v| *v <= threshold).collect()))
        }
    }

    /// The boundary `run` reports for `threshold`: doubling, then refinement.
    fn boundary_for(threshold: u32, jobs: usize) -> Option<u32> {
        let bounds = match find_ladder_bounds(jobs, threshold_probe(threshold)).unwrap() {
            Search::Done(bounds) => bounds,
            Search::Cancelled { .. } => unreachable!("no cancellation in this test"),
        };
        let (last_ok, first_failed) = bounds;
        let last_ok = last_ok?;
        let Some(hi) = first_failed else {
            return Some(last_ok);
        };
        match refine_boundary(last_ok, hi, jobs, threshold_probe(threshold)).unwrap() {
            Search::Done(boundary) => Some(boundary),
            Search::Cancelled { .. } => unreachable!("no cancellation in this test"),
        }
    }

    #[test]
    fn boundary_is_the_same_for_every_jobs_value() {
        let thresholds = [
            1,
            2,
            3,
            5,
            7,
            8,
            100,
            1000,
            SEARCH_CEILING - 1,
            SEARCH_CEILING,
            u32::MAX,
        ];
        for threshold in thresholds {
            let expected = threshold.min(SEARCH_CEILING);
            for jobs in 1..=MAX_JOBS {
                assert_eq!(
                    boundary_for(threshold, jobs),
                    Some(expected),
                    "threshold {threshold}, jobs {jobs}"
                );
            }
        }
    }

    #[test]
    fn a_failing_first_value_leaves_no_success() {
        assert_eq!(
            find_ladder_bounds(4, threshold_probe(0)).unwrap(),
            Search::Done((None, Some(1)))
        );
    }

    #[test]
    fn each_batch_uses_exactly_the_requested_concurrency() {
        for jobs in 1..=MAX_JOBS {
            let mut inner = threshold_probe(1000);
            let mut largest_batch = 0;
            let mut probe = |values: &[u32]| {
                largest_batch = largest_batch.max(values.len());
                inner(values)
            };
            let Search::Done((Some(lo), Some(hi))) = find_ladder_bounds(jobs, &mut probe).unwrap()
            else {
                panic!("expected a bracket around 1000 for jobs {jobs}");
            };
            refine_boundary(lo, hi, jobs, &mut probe).unwrap();
            assert_eq!(largest_batch, jobs, "jobs {jobs}");
        }
    }

    #[test]
    fn cancellation_mid_search_keeps_the_last_success() {
        let mut calls = 0;
        let mut probe = |values: &[u32]| -> Result<Option<Vec<bool>>, CliError> {
            calls += 1;
            if calls == 1 {
                Ok(Some(values.iter().map(|_| true).collect()))
            } else {
                Ok(None)
            }
        };
        assert_eq!(
            find_ladder_bounds(2, &mut probe).unwrap(),
            Search::Cancelled { last_ok: Some(2) }
        );
    }

    #[test]
    fn ladder_batches_double_and_stop_at_the_ceiling() {
        assert_eq!(ladder_batch(1, 1), vec![1]);
        assert_eq!(ladder_batch(1, 3), vec![1, 2, 4]);
        assert_eq!(ladder_batch(1 << 23, 4), vec![1 << 23, 1 << 24]);
        assert_eq!(ladder_batch(1 << 24, 4), vec![1 << 24]);
        assert_eq!(ladder_batch(2, 0), vec![2]);
    }

    #[test]
    fn concurrent_probes_return_results_in_input_order() {
        let values = [4, 1, 6, 3];
        let results = probe_concurrently(&values, |value| {
            // Later values finish first, so completion order differs from input order.
            std::thread::sleep(std::time::Duration::from_millis(u64::from(10 - value)));
            Ok(value % 2 == 1)
        });
        let outcomes: Vec<bool> = results.into_iter().map(|r| r.unwrap()).collect();
        assert_eq!(outcomes, vec![false, true, false, true]);
    }

    #[test]
    fn jobs_outside_one_to_eight_is_rejected_before_loading_the_contract() {
        for jobs in [0, MAX_JOBS + 1] {
            let args = LimitsArgs {
                contract: "missing.wasm".into(),
                function: "run".into(),
                ramp: "amount".into(),
                jobs,
                dry_run: false,
            };
            let out = crate::commands::output::Output {
                verbosity: crate::commands::output::Verbosity::Quiet,
                color: false,
            };
            let err = run(args, out).unwrap_err();
            assert_eq!(err.0, format!("--jobs must be between 1 and 8 (got {jobs})"));
        }
    }

    #[test]
    fn cancellation_without_success_is_an_error() {
        let args = LimitsArgs {
            contract: "test.wasm".into(),
            function: "run".into(),
            ramp: "amount".into(),
            jobs: 1,
            dry_run: false,
        };
        let err = report_interrupted(&args, None).expect_err("no successful result");
        assert!(err.0.contains("before any successful probe"));
        assert!(report_interrupted(&args, Some(7)).is_ok());
    }

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
        assert!(err.0.contains("no function named `withdraw`"), "{err}");
        assert!(err.0.contains("`vault.wasm`"), "{err}");
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
            err.0
                .contains("`batch_payout` has no parameter named `recipients`"),
            "{err}"
        );
        assert!(err.0.contains("its parameters are `funder`"), "{err}");
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
        assert!(
            err.0
                .contains("parameter `balances` has type `Map<Symbol, u32>`"),
            "{err}"
        );
        assert_no_debug_output(&err);
    }

    /// The raw `Debug` shapes the error conventions in `commands/mod.rs`
    /// rule out.
    fn assert_no_debug_output(err: &CliError) {
        for marker in ["ScSpecType", " { ", "Box(", "\\\""] {
            assert!(!err.0.contains(marker), "debug output {marker:?} in: {err}");
        }
    }

    #[test]
    fn unsupported_ramp_types_are_named_without_debug_output() {
        let env = Env::default();
        let ty = ScSpecTypeDef::Option(Box::new(ScSpecTypeOption {
            value_type: Box::new(vec_of(ScSpecTypeDef::BytesN(ScSpecTypeBytesN { n: 32 }))),
        }));
        let err = ramp_val(&env, &ty, 3).expect_err("not a ramp type");
        assert!(err.0.contains("(`Option<Vec<BytesN<32>>>`)"), "{err}");
        assert_no_debug_output(&err);
    }

    #[test]
    fn type_names_read_like_rust() {
        let cases = [
            (ScSpecTypeDef::U32, "u32"),
            (ScSpecTypeDef::Bool, "bool"),
            (vec_of(ScSpecTypeDef::Address), "Vec<Address>"),
            (
                ScSpecTypeDef::Result(Box::new(ScSpecTypeResult {
                    ok_type: Box::new(ScSpecTypeDef::Void),
                    error_type: Box::new(ScSpecTypeDef::Error),
                })),
                "Result<(), Error>",
            ),
            (
                ScSpecTypeDef::Tuple(Box::new(ScSpecTypeTuple {
                    value_types: vec![ScSpecTypeDef::I128, ScSpecTypeDef::Symbol]
                        .try_into()
                        .expect("two types fit"),
                })),
                "(i128, Symbol)",
            ),
            (
                ScSpecTypeDef::Udt(ScSpecTypeUdt {
                    name: "DataKey".try_into().expect("name fits"),
                }),
                "DataKey",
            ),
        ];
        for (ty, expected) in cases {
            assert_eq!(type_name(&ty), expected);
        }
    }

    #[test]
    fn spec_read_failures_name_the_file_and_the_fix() {
        let path = std::path::Path::new("README.md");
        let err = soroban_spec::read::from_wasm(b"not wasm").expect_err("not a wasm module");
        let err = spec_error(path, &err);
        assert!(
            err.0
                .starts_with("`README.md` is not a valid WebAssembly module"),
            "{err}"
        );
        assert!(err.0.contains("wasm32v1-none"), "{err}");
        assert_no_debug_output(&err);

        let err = spec_error(path, &soroban_spec::read::FromWasmError::NotFound);
        assert!(err.0.contains("no contract spec"), "{err}");
    }
}
