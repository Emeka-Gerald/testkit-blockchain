use std::io::IsTerminal;

use clap::Args;

/// Output controls shared by every subcommand. Declared `global` so they
/// are accepted before or after the subcommand name
/// (`soroban-testkit -q audit` and `soroban-testkit audit -q` both work).
#[derive(Args)]
pub struct GlobalArgs {
    /// Never emit ANSI color codes, here or from tools this command runs
    /// (e.g. cargo). Same as setting the `NO_COLOR` environment variable
    #[arg(long, global = true, verbatim_doc_comment)]
    pub no_color: bool,
    /// Print only results and errors; suppress advisory notes. Cannot be
    /// combined with --verbose
    #[arg(
        short,
        long,
        global = true,
        conflicts_with = "verbose",
        verbatim_doc_comment
    )]
    pub quiet: bool,
    /// Print extra diagnostics (commands run, files scanned) to stderr.
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verbosity {
    Quiet,
    Normal,
    Verbose,
}

/// How a command should write to the terminal.
///
/// Results (findings, measured limits, JSON documents) and errors are
/// always printed. `--quiet` drops advisory notes; `--verbose` adds
/// diagnostics on stderr, so stdout stays parseable at every level.
#[derive(Clone, Copy, Debug)]
pub struct Output {
    pub verbosity: Verbosity,
    /// False when the user opted out of color (`--no-color`, `NO_COLOR`).
    /// Even when true, color is only emitted to a terminal.
    pub color: bool,
}

impl Output {
    pub fn from_args(args: &GlobalArgs) -> Self {
        let verbosity = if args.quiet {
            Verbosity::Quiet
        } else if args.verbose {
            Verbosity::Verbose
        } else {
            Verbosity::Normal
        };
        let color = color_allowed(
            args.no_color,
            std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()),
        );
        Output { verbosity, color }
    }

    /// Prints an advisory note to stdout, unless `--quiet`.
    pub fn note(&self, msg: impl std::fmt::Display) {
        if self.verbosity != Verbosity::Quiet {
            println!("{msg}");
        }
    }

    /// Prints a diagnostic to stderr, only with `--verbose`.
    pub fn detail(&self, msg: impl std::fmt::Display) {
        if self.verbosity == Verbosity::Verbose {
            eprintln!("{msg}");
        }
    }

    /// The `error:` prefix `main` puts before a failure message: red on a
    /// color terminal, plain otherwise.
    pub fn error_prefix(&self) -> &'static str {
        self.error_prefix_for(std::io::stderr().is_terminal())
    }

    fn error_prefix_for(&self, is_terminal: bool) -> &'static str {
        if self.color && is_terminal {
            "\x1b[1;31merror:\x1b[0m"
        } else {
            "error:"
        }
    }
}

/// Color is allowed unless `--no-color` is passed or `NO_COLOR` is set to
/// a non-empty value (see <https://no-color.org>).
fn color_allowed(no_color_flag: bool, no_color_env: bool) -> bool {
    !no_color_flag && !no_color_env
}

/// Whether `--no-color` appears on the raw command line. Checked before
/// clap parses, so clap's own help and usage errors honor it too.
pub fn no_color_requested(args: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    args.into_iter()
        .take_while(|arg| arg != "--")
        .any(|arg| arg == "--no-color")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(verbosity: Verbosity, color: bool) -> Output {
        Output { verbosity, color }
    }

    #[test]
    fn color_is_allowed_unless_opted_out() {
        assert!(color_allowed(false, false));
        assert!(!color_allowed(true, false));
        assert!(!color_allowed(false, true));
    }

    #[test]
    fn error_prefix_is_colored_only_on_a_terminal_with_color_allowed() {
        assert!(output(Verbosity::Normal, true)
            .error_prefix_for(true)
            .contains('\x1b'));
        assert_eq!(
            output(Verbosity::Normal, true).error_prefix_for(false),
            "error:"
        );
        assert_eq!(
            output(Verbosity::Normal, false).error_prefix_for(true),
            "error:"
        );
    }

    #[test]
    fn no_color_is_detected_before_a_double_dash_only() {
        let args =
            |list: &[&str]| -> Vec<std::ffi::OsString> { list.iter().map(Into::into).collect() };
        assert!(no_color_requested(args(&[
            "soroban-testkit",
            "--no-color",
            "audit"
        ])));
        assert!(no_color_requested(args(&[
            "soroban-testkit",
            "audit",
            "--no-color"
        ])));
        assert!(!no_color_requested(args(&["soroban-testkit", "audit"])));
        assert!(!no_color_requested(args(&[
            "soroban-testkit",
            "--",
            "--no-color"
        ])));
    }
}
