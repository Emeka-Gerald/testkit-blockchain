mod commands;

use clap::{ColorChoice, CommandFactory, FromArgMatches};
use commands::{audit, coverage, limits, output::Output, Cli, Commands};

fn main() {
    let mut command = Cli::command();
    if commands::output::no_color_requested(std::env::args_os()) {
        command = command.color(ColorChoice::Never);
    }
    let cli = Cli::from_arg_matches(&command.get_matches()).unwrap_or_else(|err| err.exit());
    let out = Output::from_args(&cli.global);
    let result = match cli.command {
        Commands::Coverage(args) => coverage::run(args, out),
        Commands::Limits(args) => limits::run(args, out),
        Commands::Audit(args) => audit::run(args, out),
        Commands::LimitsProbe(args) => limits::run_probe(args),
    };
    if let Err(err) = result {
        eprintln!("{}", commands::render_error(out.error_prefix(), &err));
        std::process::exit(1);
    }
}
