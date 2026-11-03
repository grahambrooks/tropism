//! MCP server for tropism: the ruleset, asked about by a coding agent.
//!
//! Three tools, specified in `design/05-interfaces.md` as what survived the
//! product review in `design/09-product-review.md`:
//!
//! - `tropism_check` — the rules, evaluated against a file list. The same call the
//!   pre-commit hook makes, asked *before* the commit rather than at it.
//! - `tropism_rules` — the ruleset, each rule satisfied, violated, or stale.
//! - `tropism_summary` — counts per check per project, and what could not run.
//!
//! Every tool is a thin adapter over `tropism_core::pipeline`: the analysis lives
//! there and nowhere else, which is what keeps "any question answerable by the CLI
//! is answerable over MCP" true by construction. The consumer is an agent with a
//! limited context window, so lists are capped and say when they were truncated,
//! and every response states which checks did not run — an agent cannot infer
//! that a zero means "never ran".
//!
//! Transport is stdio. The server reads nothing but the repository it is pointed
//! at, and never invokes a package manager.

use camino::{Utf8Path, Utf8PathBuf};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::schemars::JsonSchema;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use tropism_core::pipeline::{self, CheckScope};
use tropism_core::report::{CheckId, CheckStatus, Finding, Severity};

/// Findings returned per `tropism_check` call unless the caller asks otherwise.
const DEFAULT_LIMIT: usize = 50;
/// Example violations carried per rule by `tropism_rules`.
const DEFAULT_EXAMPLES: usize = 3;

const INSTRUCTIONS: &str = "\
tropism enforces the architecture rules in a repository's tropism.toml, across ten \
languages, by reading source and manifests — it never builds, installs, or runs a \
package manager. Call `tropism_check` with the files you are about to change, before \
committing, to learn whether the change breaks a rule; it is the same check the \
pre-commit hook runs. Call `tropism_rules` to see what the rules are, why each \
exists, and whether it currently holds. `tropism_summary` gives counts for every \
check over the whole repository. Paths are relative to the repository root unless \
absolute.";

#[derive(Clone)]
struct Tropism {
    /// Where a call without `path` looks: the server's working directory, or the
    /// first command-line argument.
    default_root: Utf8PathBuf,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct SummaryArgs {
    /// Repository root to analyze. Defaults to the server's working directory.
    path: Option<String>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct CheckArgs {
    /// Repository root holding tropism.toml. Defaults to the server's working directory.
    path: Option<String>,
    /// Files to check, relative to the repository root or absolute. Only violations
    /// whose source end is one of these files are reported — an existing violation
    /// elsewhere is not this change's fault. Omit to check the whole repository.
    files: Option<Vec<String>>,
    /// Maximum findings to return (default 50).
    limit: Option<usize>,
    /// Findings to skip, for paging past `limit`.
    offset: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
struct RulesArgs {
    /// Repository root holding tropism.toml. Defaults to the server's working directory.
    path: Option<String>,
    /// Example violations to include per violated rule (default 3).
    examples: Option<usize>,
}

#[tool_router]
impl Tropism {
    #[tool(
        name = "tropism_check",
        description = "Evaluate the repository's architecture rules (tropism.toml) against a set \
            of changed files — the same check the pre-commit hook runs. Reports only violations \
            introduced by those files. `gate` is `fail` when the hook would block the commit, \
            `pass` when it would not, and `unavailable` when there is no ruleset to evaluate. \
            Each finding carries the rule's `reason`.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn tropism_check(&self, Parameters(args): Parameters<CheckArgs>) -> CallToolResult {
        let root = self.default_root.clone();
        blocking(move || check(&root, args)).await
    }

    #[tool(
        name = "tropism_rules",
        description = "List the repository's architecture rules from tropism.toml, each with its \
            reason and a verdict over the whole repository: `satisfied`, `violated` (with a count \
            and examples), or `stale` (the rule matched nothing, so it protects nothing).",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn tropism_rules(&self, Parameters(args): Parameters<RulesArgs>) -> CallToolResult {
        let root = self.default_root.clone();
        blocking(move || rules(&root, args)).await
    }

    #[tool(
        name = "tropism_summary",
        description = "Whole-repository counts of findings per check and per project, and which \
            checks could not run and why. Cheap orientation; returns no individual findings. \
            `module-rule`/`package-rule` and `cycle` are high-confidence. `unused-dep` and \
            `missing-dep` are advisory: they measured a high false-positive rate on real \
            JavaScript, so verify before acting on them. `version-conflict`/`diamond-dep` \
            describe the lockfile, which covers every feature and platform, not one build.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn tropism_summary(&self, Parameters(args): Parameters<SummaryArgs>) -> CallToolResult {
        let root = self.default_root.clone();
        blocking(move || summary(&root, args)).await
    }
}

#[tool_handler]
impl ServerHandler for Tropism {
    // Written by hand rather than generated, so the server identifies as `tropism`
    // at the workspace version and sends its instructions at initialization.
    fn get_info(&self) -> rmcp::model::ServerConfig {
        rmcp::model::ServerConfig::new(
            rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(rmcp::model::Implementation::new(
            "tropism",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }
}

/// Analysis is synchronous and CPU-bound, so it runs on the blocking pool rather
/// than stalling the transport.
async fn blocking(work: impl FnOnce() -> Result<Value, String> + Send + 'static) -> CallToolResult {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(value)) => CallToolResult::structured(value),
        Ok(Err(message)) => CallToolResult::structured_error(json!({ "error": message })),
        Err(panic) => CallToolResult::structured_error(json!({
            "error": format!("analysis panicked: {panic}"),
        })),
    }
}

fn scan_root(default_root: &Utf8Path, path: Option<&str>) -> Result<Utf8PathBuf, String> {
    let root = match path {
        None => default_root.to_owned(),
        Some(path) if Utf8Path::new(path).is_absolute() => Utf8PathBuf::from(path),
        Some(path) => default_root.join(path),
    };
    if !root.is_dir() {
        return Err(format!("`{root}` is not a directory"));
    }
    Ok(root)
}

fn options() -> pipeline::Options {
    pipeline::Options {
        respect_ignore: true,
        rules_path: None,
        use_rules: true,
        rules_only: false,
    }
}

fn summary(default_root: &Utf8Path, args: SummaryArgs) -> Result<Value, String> {
    let root = scan_root(default_root, args.path.as_deref())?;
    let providers = tropism_lang::registry();
    let report = pipeline::analyze(&root, &providers, &options()).map_err(|e| e.to_string())?;
    serde_json::to_value(report.summary()).map_err(|e| e.to_string())
}

fn check(default_root: &Utf8Path, args: CheckArgs) -> Result<Value, String> {
    let root = scan_root(default_root, args.path.as_deref())?;
    let providers = tropism_lang::registry();
    let options = pipeline::Options {
        rules_only: true,
        ..options()
    };
    let scope = match &args.files {
        None => CheckScope::Repository,
        Some(files) => CheckScope::Files(
            files
                .iter()
                .map(|file| pipeline::normalize_path(Utf8Path::new(file), &root))
                .collect(),
        ),
    };
    let outcome =
        pipeline::check(&root, &providers, &options, &scope).map_err(|e| e.to_string())?;

    // A rule check that never ran is reported once, with its reason: the whole
    // point of `CheckStatus` is that an empty finding list means nothing alone.
    let mut unavailable: Vec<Value> = Vec::new();
    for project in &outcome.report.projects {
        for check in CheckId::RULES {
            if let Some(
                CheckStatus::Unavailable { reason } | CheckStatus::Failed { error: reason },
            ) = project.checks.get(&check)
            {
                let entry = json!({ "check": check, "reason": reason });
                if !unavailable.contains(&entry) {
                    unavailable.push(entry);
                }
            }
        }
    }

    let findings: Vec<&Finding> = outcome.report.findings().collect();
    let gate = if outcome.rules_evaluated == 0 || outcome.report.projects.is_empty() {
        "unavailable"
    } else if findings.iter().any(|f| f.severity >= Severity::Error) {
        "fail"
    } else {
        "pass"
    };
    let page = paginate(&findings, args.offset, args.limit);

    Ok(json!({
        "schema_version": outcome.report.schema_version,
        "scan_root": outcome.report.scan_root,
        "gate": gate,
        "scope": match &outcome.scope {
            CheckScope::Repository => json!("repository"),
            CheckScope::Files(files) => json!({ "files": files }),
        },
        "widened_by_ruleset_change": outcome.widened_by_ruleset_change,
        "rules_evaluated": outcome.rules_evaluated,
        "checked_files": outcome.checked_files,
        // `null` is not zero: a scoped run does not parse the rest of the
        // repository, so it declines to count violations elsewhere.
        "violations_elsewhere": outcome.suppressed,
        "unavailable": unavailable,
        "total_findings": findings.len(),
        "offset": page.offset,
        "returned": page.items.len(),
        "truncated": page.truncated,
        "findings": page.items,
    }))
}

fn rules(default_root: &Utf8Path, args: RulesArgs) -> Result<Value, String> {
    let root = scan_root(default_root, args.path.as_deref())?;
    let providers = tropism_lang::registry();
    let examples = args.examples.unwrap_or(DEFAULT_EXAMPLES);
    match pipeline::rules(&root, &providers, &options(), examples).map_err(|e| e.to_string())? {
        Some(report) => serde_json::to_value(report).map_err(|e| e.to_string()),
        None => Ok(json!({
            "scan_root": root,
            "ruleset": null,
            "rules": [],
            "note": "no tropism.toml at the scan root, so no rules are enforced; \
                     see design/11-dependency-rules.md for the format",
        })),
    }
}

struct Page<'a> {
    offset: usize,
    items: Vec<&'a Finding>,
    truncated: bool,
}

/// Silent truncation would let an agent conclude a codebase is clean when it saw
/// the first page, so the page always says whether more exist.
fn paginate<'a>(all: &[&'a Finding], offset: Option<usize>, limit: Option<usize>) -> Page<'a> {
    let offset = offset.unwrap_or(0).min(all.len());
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let end = offset.saturating_add(limit).min(all.len());
    Page {
        offset,
        items: all[offset..end].to_vec(),
        truncated: end < all.len(),
    }
}

fn default_root() -> anyhow::Result<Utf8PathBuf> {
    let root = match std::env::args().nth(1) {
        Some(arg) if arg == "--version" || arg == "-V" => {
            println!("tropism-mcp {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!(
                "tropism-mcp {}\n\nMCP server for tropism, over stdio.\n\n\
                 Usage: tropism-mcp [REPOSITORY]\n\n\
                 REPOSITORY is the default root for tool calls that do not name one;\n\
                 it defaults to the working directory.\n\n\
                 Tools: tropism_check, tropism_rules, tropism_summary",
                env!("CARGO_PKG_VERSION")
            );
            std::process::exit(0);
        }
        Some(arg) => std::path::PathBuf::from(arg),
        None => std::env::current_dir()?,
    };
    let root =
        std::fs::canonicalize(&root).map_err(|e| anyhow::anyhow!("{}: {e}", root.display()))?;
    Utf8PathBuf::from_path_buf(root)
        .map_err(|path| anyhow::anyhow!("{} is not valid UTF-8", path.display()))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let server = Tropism {
        default_root: default_root()?,
    };
    // stdout is the protocol channel; anything diagnostic goes to stderr.
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> Utf8PathBuf {
        // The workspace root: tropism's own tropism.toml, which it satisfies.
        Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize_utf8()
            .unwrap()
    }

    #[test]
    fn check_on_this_repository_passes_and_names_its_scope() {
        let value = check(
            &repo(),
            CheckArgs {
                files: Some(vec!["crates/tropism-mcp/src/main.rs".into()]),
                ..CheckArgs::default()
            },
        )
        .unwrap();
        assert_eq!(value["gate"], "pass", "{value:#}");
        assert_eq!(value["checked_files"], 1);
        assert!(value["rules_evaluated"].as_u64().unwrap() > 0);
        // A scoped run did not look elsewhere, and must not claim it found nothing there.
        assert!(value["violations_elsewhere"].is_null());
        assert_eq!(value["truncated"], false);
    }

    #[test]
    fn check_reports_a_violation_with_the_rules_reason() {
        // demo/go is deliberately broken: main.go reaches past the api into store.
        let value = check(
            &repo().join("demo/go"),
            CheckArgs {
                files: Some(vec!["main.go".into()]),
                ..CheckArgs::default()
            },
        )
        .unwrap();
        assert_eq!(value["gate"], "fail", "{value:#}");
        let finding = value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["details"]["rule_id"] == "entrypoint-goes-through-the-api")
            .expect("the planted violation");
        assert!(
            finding["evidence"][1]["note"]
                .as_str()
                .unwrap()
                .contains("entrypoint")
        );
    }

    #[test]
    fn check_without_a_ruleset_is_unavailable_not_pass() {
        let dir = std::env::temp_dir().join(format!("tropism-mcp-norules-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("go.mod"), "module example.com/x\n\ngo 1.22\n").unwrap();
        std::fs::write(dir.join("main.go"), "package main\n\nfunc main() {}\n").unwrap();
        let root = Utf8PathBuf::from_path_buf(dir.clone()).unwrap();

        let value = check(&root, CheckArgs::default());
        std::fs::remove_dir_all(&dir).unwrap();
        let value = value.unwrap();
        assert_eq!(value["gate"], "unavailable", "{value:#}");
        assert!(
            !value["unavailable"].as_array().unwrap().is_empty(),
            "{value:#}"
        );
    }

    #[test]
    fn rules_reports_every_rule_with_a_verdict_and_reason() {
        let value = rules(&repo(), RulesArgs::default()).unwrap();
        let rules = value["rules"].as_array().unwrap();
        let independent = rules
            .iter()
            .find(|r| r["id"] == "surfaces-are-independent")
            .expect("tropism.toml declares it");
        assert_eq!(independent["verdict"], "satisfied", "{independent:#}");
        assert_eq!(independent["kind"], "independent");
        assert!(independent["reason"].as_str().unwrap().contains("adapters"));
    }

    #[test]
    fn summary_states_what_did_not_run() {
        let value = summary(&repo(), SummaryArgs::default()).unwrap();
        assert!(value["project_count"].as_u64().unwrap() >= 4, "{value:#}");
        assert!(value["totals"].get("module-rule").is_some());
        assert!(value["not_run"].is_array());
    }

    #[test]
    fn a_missing_root_is_an_error_not_an_empty_report() {
        let error = summary(
            &repo(),
            SummaryArgs {
                path: Some("does/not/exist".into()),
            },
        )
        .unwrap_err();
        assert!(error.contains("not a directory"), "{error}");
    }

    #[test]
    fn pagination_says_when_it_truncated() {
        let finding = Finding::new(
            CheckId::ModuleRule,
            &Utf8PathBuf::new(),
            &["x"],
            Severity::Error,
            tropism_core::report::Confidence::High,
            "m",
        );
        let all = vec![&finding; 5];
        let page = paginate(&all, Some(1), Some(2));
        assert_eq!(
            (page.offset, page.items.len(), page.truncated),
            (1, 2, true)
        );
        let page = paginate(&all, Some(3), Some(10));
        assert_eq!((page.items.len(), page.truncated), (2, false));
        let page = paginate(&all, Some(99), None);
        assert_eq!(
            (page.offset, page.items.len(), page.truncated),
            (5, 0, false)
        );
    }
}
