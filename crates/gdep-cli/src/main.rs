//! gdep command-line interface.
//!
//! A thin adapter over `gdep-core`: parse arguments, run the analysis, render.
//! Any question answerable here is answerable over MCP with the same result.

mod render;

use std::io::IsTerminal;
use std::process::ExitCode;

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};
use gdep_core::pipeline;
use gdep_core::report::Severity;

/// Exit codes are the CI contract: a broken invocation must never look like a
/// passing build. See `design/05-interfaces.md`.
const EXIT_CLEAN: u8 = 0;
const EXIT_FINDINGS: u8 = 1;
const EXIT_ERROR: u8 = 2;

#[derive(Parser)]
#[command(
    name = "gdep",
    version,
    about = "Analyze a codebase for module and dependency problems"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run all available checks over a tree.
    Analyze(AnalyzeArgs),
}

#[derive(Args)]
struct AnalyzeArgs {
    /// Directory to scan.
    #[arg(default_value = ".")]
    path: Utf8PathBuf,

    #[arg(long, value_enum, default_value_t = Format::Auto)]
    format: Format,

    /// Lowest severity that sets the failure exit code.
    #[arg(long, default_value = "error", value_parser = parse_severity)]
    fail_on: Severity,

    /// Do not honour .gitignore.
    #[arg(long)]
    no_ignore: bool,

    /// Ruleset to enforce. Defaults to gdep.toml at the scan root.
    #[arg(long)]
    rules: Option<Utf8PathBuf>,

    /// Skip the ruleset entirely.
    #[arg(long)]
    no_rules: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum Format {
    /// Diagnostics on a terminal, JSON when piped. Safe everywhere.
    Auto,
    /// rustc/clippy-style diagnostics with source snippets.
    Text,
    /// The machine-readable contract, shared with the MCP server.
    Json,
    /// Interactive browser. Requires a terminal; cannot be piped.
    #[cfg(feature = "tui")]
    Tui,
}

fn parse_severity(raw: &str) -> Result<Severity, String> {
    raw.parse()
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Analyze(args) => match analyze(args) {
            Ok(code) => ExitCode::from(code),
            Err(error) => {
                anstream::eprintln!("gdep: {error:#}");
                ExitCode::from(EXIT_ERROR)
            }
        },
    }
}

fn analyze(args: AnalyzeArgs) -> anyhow::Result<u8> {
    let providers = gdep_lang::registry();
    let options = pipeline::Options {
        respect_ignore: !args.no_ignore,
        rules_path: args.rules.clone(),
        use_rules: !args.no_rules,
    };
    let report = pipeline::analyze(&args.path, &providers, &options)?;

    // `auto` never resolves to the TUI: it has to stay safe under a pipe, in CI, and
    // under a redirect. The interactive browser is always an explicit request.
    match args.format {
        Format::Json => println!("{}", report.to_json_pretty()?),
        Format::Text => anstream::print!("{}", render::text::render(&report)),
        Format::Auto => {
            if std::io::stdout().is_terminal() {
                anstream::print!("{}", render::text::render(&report));
            } else {
                println!("{}", report.to_json_pretty()?);
            }
        }
        #[cfg(feature = "tui")]
        Format::Tui => render::tui::run(&report)?,
    }

    let triggered = report.max_severity().is_some_and(|max| max >= args.fail_on);
    Ok(if triggered { EXIT_FINDINGS } else { EXIT_CLEAN })
}
