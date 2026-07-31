//! gdep command-line interface.
//!
//! A thin adapter over `gdep-core`: parse arguments, run the analysis, render.
//! Any question answerable here is answerable over MCP with the same result.

mod render;

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::process::ExitCode;

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};
use gdep_core::discovery::discover;
use gdep_core::report::{CheckId, CheckStatus, ProjectReport, Report, Severity};

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
    let projects = discover(&args.path, &providers, !args.no_ignore)?;

    let mut report = Report::new(args.path.clone());
    for project in projects {
        let mut project_report = ProjectReport::new(project);
        project_report.checks = pending_checks();
        report.projects.push(project_report);
    }
    report.finalize();

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

/// Until the analyzers land, every check is honestly reported as unavailable.
///
/// This is the same mechanism a missing lockfile uses at runtime, so the "check did
/// not run" path is exercised from the very first commit rather than bolted on once
/// something already depends on silence meaning success.
fn pending_checks() -> BTreeMap<CheckId, CheckStatus> {
    CheckId::ALL
        .into_iter()
        .map(|check| {
            (
                check,
                CheckStatus::unavailable("analyzer not implemented yet"),
            )
        })
        .collect()
}
