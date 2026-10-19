use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use clap::Args;
use quote::ToTokens;
use syn::visit::{self, Visit};
use syn::{BinOp, Expr, FnArg, ImplItemFn, ItemFn, Local, Pat, Signature, Type};
use walkdir::WalkDir;

use super::output::Output;
use super::CliError;

/// Arguments for `soroban-testkit audit`.
#[derive(Args)]
#[command(after_help = "EXAMPLES:\n    \
    soroban-testkit audit ./src\n    \
    soroban-testkit audit ./src --strict\n    \
    soroban-testkit audit ./src --json | jq '.findings'")]
pub struct AuditArgs {
    /// Directory or .rs file to audit; directories are scanned recursively
    /// for .rs files. Pass `-` to read a single source file from stdin
    /// (for editor integrations that audit unsaved buffers)
    #[arg(value_name = "PATH", default_value = ".", verbatim_doc_comment)]
    path: PathBuf,
    /// Exit 1 if any findings are reported. Without it, findings are
    /// printed and the command still exits 0
    #[arg(long, verbatim_doc_comment)]
    strict: bool,
    /// Print findings as a JSON document (`schema_version` 1) on stdout
    /// instead of text
    #[arg(long, verbatim_doc_comment)]
    json: bool,
}

/// Version of the `audit --json` document shape. Bump it on any breaking
/// change: a removed or renamed field, or a changed type or meaning.
/// Adding a new field is not breaking and keeps the same version.
const AUDIT_JSON_SCHEMA_VERSION: u32 = 1;

/// The bounded claim every non-quiet `audit` run ends with, including a
/// clean one: an empty result is not a security guarantee.
const DISCLAIMER: &str = "soroban-testkit audit is a linter with three heuristics, not a security \
                          product — a missing finding is not a security guarantee.";

struct Finding {
    /// See [`finding_path`].
    file: String,
    line: usize,
    rule: &'static str,
    severity: &'static str,
    message: String,
}

/// Static checks over a contract crate. **Not a security product** — a
/// linter with three heuristics, each independently useful and each
/// capable of missing real bugs or flagging non-bugs. The absence of a
/// finding here is never a security guarantee.
///
/// 1. Entry points taking an `Address` parameter that never call
///    `require_auth` anywhere in the function body.
/// 2. Arithmetic (`+ - * /`) on an explicitly `i128`-typed binding, done
///    outside a `checked_`/`saturating_`/`wrapping_` call.
/// 3. A `.persistent()`/`.temporary()` storage read with no
///    `.extend_ttl(` call anywhere in the same function.
///
/// Finding paths are relative to the audited `PATH` and `/`-separated (see
/// [`finding_path`]), so the same crate reports the same paths whichever
/// directory the command runs from.
pub fn run(args: AuditArgs, out: Output) -> Result<(), CliError> {
    let mut findings = Vec::new();

    if args.path == Path::new("-") {
        let mut src = String::new();
        std::io::stdin()
            .read_to_string(&mut src)
            .map_err(|err| CliError(format!("failed to read stdin: {err}")))?;
        audit_source(&src, "<stdin>", &mut findings);
    } else {
        // WalkDir yields a missing root as an Err entry, which the
        // filter_map below would drop, so a typo'd PATH would otherwise
        // "pass" with no findings.
        std::fs::metadata(&args.path).map_err(|err| {
            CliError(format!(
                "cannot audit {}: {err}; pass a contract crate's source directory, \
                 a .rs file, or `-` for stdin",
                super::quote(args.path.display())
            ))
        })?;
        for entry in WalkDir::new(&args.path).into_iter().filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.path().extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            out.detail(format!("scanning {}", entry.path().display()));
            audit_file(
                entry.path(),
                &finding_path(&args.path, entry.path()),
                &mut findings,
            )?;
        }
    }

    if args.json {
        println!("{}", render_json(&findings));
    } else if findings.is_empty() {
        out.note(format!("audit: no findings. {DISCLAIMER}"));
    } else {
        for finding in &findings {
            println!(
                "{}:{}: [{}] {}: {}",
                finding.file, finding.line, finding.severity, finding.rule, finding.message
            );
        }
        out.note(format!("\n{} finding(s). {DISCLAIMER}", findings.len()));
    }

    if args.strict && !findings.is_empty() {
        return Err(CliError(format!(
            "{} audit finding(s) (--strict)",
            findings.len()
        )));
    }
    Ok(())
}

/// Renders findings as the `audit --json` document, schema version
/// [`AUDIT_JSON_SCHEMA_VERSION`]:
///
/// ```json
/// {"schema_version":1,"disclaimer":"...","finding_count":1,"findings":[{"file":"src/lib.rs","line":7,"severity":"warning","rule":"missing-require-auth","message":"..."}]}
/// ```
fn render_json(findings: &[Finding]) -> String {
    let items: Vec<String> = findings
        .iter()
        .map(|finding| {
            format!(
                "{{\"file\":{},\"line\":{},\"severity\":{},\"rule\":{},\"message\":{}}}",
                json_string(&finding.file),
                finding.line,
                json_string(finding.severity),
                json_string(finding.rule),
                json_string(&finding.message),
            )
        })
        .collect();
    format!(
        "{{\"schema_version\":{AUDIT_JSON_SCHEMA_VERSION},\"disclaimer\":{},\"finding_count\":{},\"findings\":[{}]}}",
        json_string(DISCLAIMER),
        findings.len(),
        items.join(",")
    )
}

/// Quotes and escapes `value` as a JSON string. Hand-rolled so the audit
/// command takes no serialization dependency for one small document.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The path a finding in `file` is reported under: relative to the audited
/// `root` with `/` separators, e.g. `src/lib.rs` for `root = "."` or
/// `lib.rs` for `root = "examples/vault/src"`. When `root` is the file
/// itself, its file name is used. Falls back to `file` as given if it is
/// not under `root`.
fn finding_path(root: &Path, file: &Path) -> String {
    let relative = match file.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => file.file_name().map(Path::new).unwrap_or(file),
        Ok(rel) => rel,
        Err(_) => file,
    };
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn audit_file(path: &Path, display: &str, findings: &mut Vec<Finding>) -> Result<(), CliError> {
    let src = std::fs::read_to_string(path).map_err(|err| {
        CliError(format!(
            "failed to read {}: {err}",
            super::quote(path.display())
        ))
    })?;
    audit_source(&src, display, findings);
    Ok(())
}

/// Parses `src` (the contents of a single `.rs` file, attributed to
/// `display` in findings) and walks it for audit findings. An empty or
/// unparseable source produces zero findings rather than an error —
/// not every `.rs` file under a crate root need parse standalone, and
/// a stdin editor buffer may be mid-edit.
fn audit_source(src: &str, display: &str, findings: &mut Vec<Finding>) {
    let file = match syn::parse_file(src) {
        Ok(file) => file,
        Err(_) => return,
    };

    let mut visitor = FunctionVisitor {
        path: display.to_string(),
        findings,
    };
    visitor.visit_file(&file);
}

struct FunctionVisitor<'a> {
    path: String,
    findings: &'a mut Vec<Finding>,
}

impl<'ast> Visit<'ast> for FunctionVisitor<'_> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        let line = line_of(node);
        self.audit_function(&node.sig, node.to_token_stream().to_string(), line);
        self.audit_i128_arithmetic(&node.sig, &node.block, line);
        visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        let line = line_of(node);
        self.audit_function(&node.sig, node.to_token_stream().to_string(), line);
        self.audit_i128_arithmetic(&node.sig, &node.block, line);
        visit::visit_impl_item_fn(self, node);
    }
}

impl FunctionVisitor<'_> {
    fn audit_function(&mut self, sig: &Signature, body_src: String, line: usize) {
        let name = sig.ident.to_string();

        // Rule 1: an Address parameter that's never checked with require_auth.
        let address_params = address_typed_params(sig);
        if !address_params.is_empty() && !body_src.contains("require_auth") {
            self.push(
                line,
                "missing-require-auth",
                "warning",
                format!(
                    "fn {name} takes Address parameter(s) {address_params:?} but never calls \
                     require_auth in its body"
                ),
            );
        }

        // Rule 3: a storage read with no extend_ttl anywhere in the function.
        let reads_persistent = body_src.contains(". persistent () . get (");
        let reads_temporary = body_src.contains(". temporary () . get (");
        if (reads_persistent || reads_temporary) && !body_src.contains(". extend_ttl (") {
            self.push(
                line,
                "missing-ttl-bump",
                "warning",
                format!(
                    "fn {name} reads from {} storage but never calls extend_ttl in its body",
                    if reads_persistent {
                        "persistent"
                    } else {
                        "temporary"
                    }
                ),
            );
        }
    }

    fn push(&mut self, line: usize, rule: &'static str, severity: &'static str, message: String) {
        self.findings.push(Finding {
            file: self.path.clone(),
            line,
            rule,
            severity,
            message,
        });
    }

    fn audit_i128_arithmetic(&mut self, sig: &Signature, body: &syn::Block, line: usize) {
        let mut i128_bindings: HashSet<String> = HashSet::new();
        for arg in &sig.inputs {
            if let FnArg::Typed(pat_type) = arg {
                if type_is_i128(&pat_type.ty) {
                    if let Pat::Ident(ident) = &*pat_type.pat {
                        i128_bindings.insert(ident.ident.to_string());
                    }
                }
            }
        }

        let mut visitor = I128ArithmeticVisitor {
            path: self.path.clone(),
            fn_name: sig.ident.to_string(),
            fn_line: line,
            i128_bindings,
            findings: self.findings,
        };
        visitor.visit_block(body);
    }
}

fn address_typed_params(sig: &Signature) -> Vec<String> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pat_type) if type_is_address(&pat_type.ty) => match &*pat_type.pat {
                Pat::Ident(ident) => Some(ident.ident.to_string()),
                _ => Some("<pattern>".to_string()),
            },
            _ => None,
        })
        .collect()
}

fn type_is_address(ty: &Type) -> bool {
    match ty {
        Type::Reference(r) => type_is_address(&r.elem),
        Type::Path(p) => p
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Address"),
        _ => false,
    }
}

fn line_of(node: impl quote::ToTokens) -> usize {
    node.to_token_stream()
        .into_iter()
        .next()
        .map(|t| t.span().start().line)
        .unwrap_or(0)
}

/// Rule 2 uses a dedicated AST walk (rather than a text search) because
/// "arithmetic on an i128" needs to know which bindings are actually
/// typed `i128`, and a plain string search over the token stream can't
/// tell an `i128` value from the substring appearing anywhere else.
struct I128ArithmeticVisitor<'a> {
    path: String,
    fn_name: String,
    fn_line: usize,
    i128_bindings: HashSet<String>,
    findings: &'a mut Vec<Finding>,
}

impl<'ast> Visit<'ast> for I128ArithmeticVisitor<'_> {
    fn visit_local(&mut self, node: &'ast Local) {
        if let Pat::Type(pat_type) = &node.pat {
            if type_is_i128(&pat_type.ty) {
                if let Pat::Ident(ident) = &*pat_type.pat {
                    self.i128_bindings.insert(ident.ident.to_string());
                }
            }
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        let is_arith = matches!(
            node.op,
            BinOp::Add(_) | BinOp::Sub(_) | BinOp::Mul(_) | BinOp::Div(_)
        );
        if is_arith && (self.touches_i128(&node.left) || self.touches_i128(&node.right)) {
            self.findings.push(Finding {
                file: self.path.clone(),
                line: self.fn_line,
                rule: "unchecked-i128-arithmetic",
                severity: "warning",
                message: format!(
                    "fn {} does raw arithmetic on an i128 value; prefer checked_/saturating_/ \
                     wrapping_ variants to avoid silent overflow",
                    self.fn_name
                ),
            });
        }
        visit::visit_expr_binary(self, node);
    }
}

impl I128ArithmeticVisitor<'_> {
    fn touches_i128(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Path(p) => p
                .path
                .get_ident()
                .is_some_and(|ident| self.i128_bindings.contains(&ident.to_string())),
            Expr::Lit(lit) => matches!(&lit.lit, syn::Lit::Int(i) if i.suffix() == "i128"),
            Expr::Paren(p) => self.touches_i128(&p.expr),
            Expr::Group(g) => self.touches_i128(&g.expr),
            _ => false,
        }
    }
}

fn type_is_i128(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.is_ident("i128"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_help_states_the_current_schema_version() {
        // `audit --help` documents `--json` as "(`schema_version` 1)"; bump
        // that text (and the help snapshot) together with the constant.
        assert_eq!(AUDIT_JSON_SCHEMA_VERSION, 1);
    }

    #[test]
    fn json_document_declares_schema_version_and_findings() {
        let findings = vec![Finding {
            file: "src/lib.rs".to_string(),
            line: 7,
            rule: "missing-require-auth",
            severity: "warning",
            message: "fn pay takes Address parameter(s) [\"from\"]".to_string(),
        }];
        let json = render_json(&findings);
        assert!(json.starts_with("{\"schema_version\":1,"), "{json}");
        assert!(json.contains("\"finding_count\":1"), "{json}");
        assert!(json.contains("\"line\":7"), "{json}");
        assert!(json.contains("\"rule\":\"missing-require-auth\""), "{json}");
        assert!(json.contains(r#"[\"from\"]"#), "{json}");
    }

    #[test]
    fn json_document_for_no_findings_is_an_empty_list() {
        let json = render_json(&[]);
        assert!(
            json.ends_with(",\"finding_count\":0,\"findings\":[]}"),
            "{json}"
        );
    }

    #[test]
    fn finding_paths_are_relative_to_the_audited_root() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(finding_path(&p("."), &p("./src/lib.rs")), "src/lib.rs");
        assert_eq!(
            finding_path(&p("examples/vault/src"), &p("examples/vault/src/lib.rs")),
            "lib.rs"
        );
        assert_eq!(
            finding_path(&p("/abs/crate"), &p("/abs/crate/src/nested/mod.rs")),
            "src/nested/mod.rs"
        );
    }

    #[test]
    fn a_single_file_root_reports_its_file_name() {
        let file = PathBuf::from("crates/cli/src/commands/mod.rs");
        assert_eq!(finding_path(&file, &file), "mod.rs");
    }

    #[test]
    fn a_file_outside_the_root_keeps_its_given_path() {
        assert_eq!(
            finding_path(Path::new("src"), Path::new("other/lib.rs")),
            "other/lib.rs"
        );
    }

    #[test]
    fn json_strings_escape_quotes_backslashes_and_control_characters() {
        let expected = "\"a\\\"b\\\\c\\nd\\u0001\"";
        assert_eq!(json_string("a\"b\\c\nd\u{1}"), expected);
    }
}
