//! tropism command-line interface.
//!
//! A thin adapter over `tropism-core`: parse arguments, run the analysis, render.
//! Any question answerable here is answerable over MCP with the same result.

mod git;
mod render;

use std::io::IsTerminal;
use std::process::ExitCode;

use camino::Utf8PathBuf;
use clap::{Args, Parser, Subcommand, ValueEnum};
use tropism_core::pipeline::{self, CheckScope};
use tropism_core::report::Severity;

/// Exit codes are the CI contract: a broken invocation must never look like a
/// passing build. See `design/05-interfaces.md`.
const EXIT_CLEAN: u8 = 0;
const EXIT_FINDINGS: u8 = 1;
const EXIT_ERROR: u8 = 2;

#[derive(Parser)]
#[command(
    name = "tropism",
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

    /// Check files against the ruleset. Fast enough for a pre-commit hook.
    ///
    /// Runs only `module-rule` and `package-rule` — the checks sound enough to
    /// block a commit. With a file list, reports only violations those files
    /// introduce, which ratchets an already-violating codebase without a baseline.
    Check(CheckArgs),
}

#[derive(Args)]
struct CheckArgs {
    /// Files to check, relative to the scan root. Empty means the whole repository.
    ///
    /// The primary interface: every hook framework already passes changed files as
    /// arguments, so this is the form the ecosystem speaks. `--staged` and
    /// `--since` are sugar over it.
    files: Vec<Utf8PathBuf>,

    /// Check the files staged for commit.
    #[arg(long, conflicts_with_all = ["files", "since"])]
    staged: bool,

    /// Check the files changed since a ref, against the merge base.
    ///
    /// For CI on a pull request: `--since origin/main` reports what the branch
    /// introduced rather than everything already wrong on it.
    #[arg(long, value_name = "REF", conflicts_with = "files")]
    since: Option<String>,

    /// Directory to scan.
    #[arg(long, default_value = ".")]
    path: Utf8PathBuf,

    #[arg(long, value_enum, default_value_t = Format::Auto)]
    format: Format,

    /// Lowest severity that sets the failure exit code.
    #[arg(long, default_value = "error", value_parser = parse_severity)]
    fail_on: Severity,

    /// Do not honour .gitignore.
    #[arg(long)]
    no_ignore: bool,

    /// Ruleset to enforce. Defaults to tropism.toml at the scan root.
    #[arg(long)]
    rules: Option<Utf8PathBuf>,
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

    /// Ruleset to enforce. Defaults to tropism.toml at the scan root.
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
        Command::Analyze(args) => run(analyze(args)),
        Command::Check(args) => run(check(args)),
    }
}

fn run(result: anyhow::Result<u8>) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            anstream::eprintln!("tropism: {error:#}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn check(args: CheckArgs) -> anyhow::Result<u8> {
    let providers = tropism_lang::registry();
    let options = pipeline::Options {
        respect_ignore: !args.no_ignore,
        rules_path: args.rules.clone(),
        use_rules: true,
        rules_only: true,
    };

    // `--staged` and `--since` produce a file list; everything downstream then
    // treats all three forms identically.
    let listed = if args.staged {
        Some(git::staged_files(&args.path)?)
    } else if let Some(base) = &args.since {
        Some(git::files_since(&args.path, base)?)
    } else if args.files.is_empty() {
        None
    } else {
        Some(args.files.clone())
    };

    // An empty list is not the same as no list. `--staged` with nothing staged has
    // genuinely nothing to check and must not silently widen to the repository —
    // that would turn a no-op commit into a full run, and on a large repository
    // into a slow one. A bare `tropism check` with no flags is the whole repository.
    let scope = match listed {
        None => CheckScope::Repository,
        Some(files) => CheckScope::Files(
            files
                .iter()
                .map(|file| normalize(file, &args.path))
                .collect(),
        ),
    };

    let outcome = pipeline::check(&args.path, &providers, &options, &scope)?;

    match args.format {
        Format::Json => println!("{}", outcome.report.to_json_pretty()?),
        Format::Text => anstream::print!("{}", render::text::render_check(&outcome)),
        Format::Auto => {
            if std::io::stdout().is_terminal() {
                anstream::print!("{}", render::text::render_check(&outcome));
            } else {
                println!("{}", outcome.report.to_json_pretty()?);
            }
        }
        #[cfg(feature = "tui")]
        Format::Tui => render::tui::run(&outcome.report)?,
    }

    let triggered = outcome
        .report
        .max_severity()
        .is_some_and(|max| max >= args.fail_on);
    Ok(if triggered { EXIT_FINDINGS } else { EXIT_CLEAN })
}

/// A path as the report spells it: relative to the scan root, `/`-separated.
///
/// Hook frameworks pass repository-relative paths and humans pass whatever their
/// shell completed, so `./src/a.rs`, `src/a.rs`, and an absolute path all have to
/// name the same file. A path that cannot be made relative is passed through
/// unchanged — it will simply match nothing, which is the right outcome for a file
/// outside the scan root.
fn normalize(file: &Utf8PathBuf, scan_root: &Utf8PathBuf) -> Utf8PathBuf {
    let relative = file
        .strip_prefix(scan_root)
        .or_else(|_| file.strip_prefix("./"))
        .unwrap_or(file);
    Utf8PathBuf::from(relative.as_str().replace('\\', "/"))
}

fn analyze(args: AnalyzeArgs) -> anyhow::Result<u8> {
    let providers = tropism_lang::registry();
    let options = pipeline::Options {
        respect_ignore: !args.no_ignore,
        rules_path: args.rules.clone(),
        use_rules: !args.no_rules,
        rules_only: false,
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
