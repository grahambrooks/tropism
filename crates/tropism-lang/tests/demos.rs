//! Tests over the `demo/` sample projects.
//!
//! The demos are the tool's shop window, so they have to keep demonstrating what
//! their READMEs claim. These assert both halves: every planted problem is found,
//! and every planted trap stays silent.

use camino::Utf8PathBuf;
use tropism_core::pipeline::{self, Options};
use tropism_core::report::{CheckId, CheckStatus, Finding, Report};

fn demo(name: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("demo")
        .join(name)
}

fn analyze(name: &str) -> Report {
    let providers = tropism_lang::registry();
    pipeline::analyze(&demo(name), &providers, &Options::default()).expect("analysis failed")
}

fn messages(report: &Report, check: CheckId) -> Vec<String> {
    report
        .findings()
        .filter(|finding| finding.check == check)
        .map(|finding| finding.message.clone())
        .collect()
}

fn mentions(report: &Report, check: CheckId, needle: &str) -> bool {
    messages(report, check)
        .iter()
        .any(|message| message.contains(needle))
}

/// Nothing anywhere in a demo may reference a package the README does not mention
/// as planted. This is what stops a demo quietly acquiring false positives.
fn assert_no_unexpected(report: &Report, expected: &[&str]) {
    for finding in report.findings() {
        assert!(
            expected
                .iter()
                .any(|needle| finding.message.contains(needle)),
            "unexpected finding: [{}] {}",
            finding.check,
            finding.message
        );
    }
}

// --- Go ---------------------------------------------------------------------

#[test]
fn go_demo_finds_both_planted_problems() {
    let report = analyze("go");
    assert!(mentions(&report, CheckId::UnusedDep, "golang.org/x/sync"));
    assert!(mentions(
        &report,
        CheckId::MissingDep,
        "github.com/rs/zerolog"
    ));
}

/// A blank import exists for its side effects, and an `// indirect` entry is not
/// expected to be imported at all.
#[test]
fn go_demo_does_not_trip_on_its_traps() {
    let report = analyze("go");
    for trap in ["lib/pq", "testify"] {
        assert!(
            !mentions(&report, CheckId::UnusedDep, trap),
            "tripped on {trap}"
        );
    }
}

#[test]
fn go_demo_reports_the_resolved_tree_as_structurally_unavailable() {
    let report = analyze("go");
    match &report.projects[0].checks[&CheckId::VersionConflict] {
        CheckStatus::Unavailable { reason } => assert!(reason.contains("go.sum"), "{reason}"),
        other => panic!("expected unavailable, got {other:?}"),
    }
}

#[test]
fn go_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("go"),
        &[
            "golang.org/x/sync",
            "github.com/rs/zerolog",
            "entrypoint-goes-through-the-api",
        ],
    );
}

// --- JavaScript -------------------------------------------------------------

/// The only ecosystem here with a genuinely resolved lockfile, so this is the one
/// demo where everything except the deferred check runs.
#[test]
fn javascript_demo_runs_every_check_but_the_deferred_one() {
    let report = analyze("javascript");
    let unavailable: Vec<CheckId> = report.projects[0]
        .checks
        .iter()
        .filter(|(_, status)| matches!(status, CheckStatus::Unavailable { .. }))
        .map(|(check, _)| *check)
        .collect();
    assert_eq!(
        unavailable,
        vec![CheckId::DependencyBloat],
        "got {unavailable:?}"
    );
}

#[test]
fn javascript_demo_finds_every_planted_problem() {
    let report = analyze("javascript");
    assert_eq!(
        messages(&report, CheckId::Cycle).len(),
        1,
        "the file-level cycle"
    );
    assert!(mentions(&report, CheckId::UnusedDep, "left-pad"));
    assert!(mentions(&report, CheckId::MissingDep, "chalk"));
    assert!(mentions(&report, CheckId::VersionConflict, "ms"));
    assert!(mentions(&report, CheckId::DiamondDep, "ms"));
}

/// Node builtins need no declaration, and tooling packages are never imported.
///
/// Scoped to the hygiene checks on purpose: `vitest` legitimately appears in the
/// diamond finding as one of the dependents that disagreed about `ms`.
#[test]
fn javascript_demo_does_not_trip_on_its_traps() {
    let report = analyze("javascript");
    for trap in ["node:fs", "@types/node", "eslint", "vitest"] {
        for check in [CheckId::UnusedDep, CheckId::MissingDep] {
            assert!(!mentions(&report, check, trap), "{check} tripped on {trap}");
        }
    }
}

#[test]
fn javascript_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("javascript"),
        &["cycle", "left-pad", "chalk", "ms", "lodash"],
    );
}

// --- Rust -------------------------------------------------------------------

/// Rust permits module cycles, unlike Go, so nothing in the toolchain catches
/// this one.
#[test]
fn rust_demo_finds_the_module_cycle() {
    let report = analyze("rust");
    let cycles = messages(&report, CheckId::Cycle);
    assert_eq!(cycles.len(), 1, "got {cycles:?}");
    assert!(cycles[0].contains("2 modules"));
}

#[test]
fn rust_demo_finds_the_hygiene_problems() {
    let report = analyze("rust");
    assert!(mentions(&report, CheckId::UnusedDep, "once_cell"));
    assert!(mentions(&report, CheckId::MissingDep, "serde_json"));
}

#[test]
fn rust_demo_finds_the_duplicated_crate_in_the_lockfile() {
    let report = analyze("rust");
    assert!(mentions(&report, CheckId::VersionConflict, "libc"));
    assert!(mentions(&report, CheckId::DiamondDep, "libc"));
}

/// Each of these tripped tropism during the dogfood run, and each is a false positive
/// this demo exists to keep fixed:
///
/// * `anyhow` is used only as a fully-qualified path, never imported.
/// * `thiserror` appears only inside a derive attribute's token tree.
/// * `use super::*` in a `#[cfg(test)]` module is containment, not a cycle.
#[test]
fn rust_demo_does_not_trip_on_its_traps() {
    let report = analyze("rust");
    for trap in ["anyhow", "thiserror", "engine"] {
        assert!(
            !mentions(&report, CheckId::UnusedDep, trap),
            "reported {trap} unused; it is used without a `use` statement"
        );
    }
}

#[test]
fn rust_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("rust"),
        &[
            "cycle",
            "regex",
            "serde_json",
            "libc",
            "once_cell",
            "independent",
        ],
    );
}

// --- .NET -------------------------------------------------------------------

#[test]
fn dotnet_demo_discovers_every_project_by_csproj_extension() {
    let report = analyze("dotnet");
    assert_eq!(
        report.projects.len(),
        4,
        "manifests are named after the project"
    );
}

/// C# permits namespace cycles, so nothing in the toolchain catches this. Scoped
/// to `module`, since this demo also carries a project-scoped cycle.
#[test]
fn dotnet_demo_finds_the_namespace_cycle() {
    let report = analyze("dotnet");
    let module_cycles: Vec<&Finding> = report
        .findings()
        .filter(|f| f.check == CheckId::Cycle && f.details["scope"] == "module")
        .collect();
    assert_eq!(
        module_cycles.len(),
        1,
        "got {:?}",
        messages(&report, CheckId::Cycle)
    );
}

#[test]
fn dotnet_demo_finds_the_hygiene_problems() {
    let report = analyze("dotnet");
    assert!(mentions(&report, CheckId::UnusedDep, "AutoMapper"));
    assert!(mentions(&report, CheckId::MissingDep, "Serilog"));
}

/// A `using` names a namespace, so the framework, the solution's own code, and a
/// test project all have to be told apart from packages.
#[test]
fn dotnet_demo_does_not_trip_on_its_traps() {
    let report = analyze("dotnet");
    for trap in ["System", "StyleCop", "Shop.Domain.Orders", "xunit"] {
        for check in [CheckId::UnusedDep, CheckId::MissingDep] {
            assert!(!mentions(&report, check, trap), "{check} tripped on {trap}");
        }
    }
}

/// packages.lock.json is opt-in and usually absent, so most .NET solutions get
/// the same treatment Java will.
#[test]
fn dotnet_demo_reports_resolved_tree_checks_as_unavailable() {
    let report = analyze("dotnet");
    for project in &report.projects {
        assert!(matches!(
            project.checks[&CheckId::VersionConflict],
            CheckStatus::Unavailable { .. }
        ));
    }
}

/// The gap that made the per-project graph misleading: `Shop.Domain` and
/// `Shop.Data` reference each other, and until cycle detection ran repo-wide the
/// check reported `ok` while the two packages were mutually dependent.
#[test]
fn dotnet_demo_finds_the_cross_project_cycle() {
    let report = analyze("dotnet");
    let project_cycles: Vec<&Finding> = report
        .findings()
        .filter(|f| f.check == CheckId::Cycle && f.details["scope"] == "project")
        .collect();

    assert_eq!(
        project_cycles.len(),
        1,
        "got {:?}",
        messages(&report, CheckId::Cycle)
    );
    let members = project_cycles[0].details["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert!(project_cycles[0].message.contains("Shop.Data"));
    assert!(project_cycles[0].message.contains("Shop.Domain"));

    // Both arms are cited, so each half of the cycle is checkable.
    assert_eq!(project_cycles[0].evidence.len(), 2);
}

/// The two scopes are distinct findings about distinct problems: the namespace
/// cycle lives inside one project, the project cycle spans two.
#[test]
fn dotnet_demo_reports_both_cycle_scopes() {
    let report = analyze("dotnet");
    let scopes: Vec<&str> = report
        .findings()
        .filter(|f| f.check == CheckId::Cycle)
        .filter_map(|f| f.details["scope"].as_str())
        .collect();
    assert!(scopes.contains(&"module"), "got {scopes:?}");
    assert!(scopes.contains(&"project"), "got {scopes:?}");
}

/// A single-project repository cannot have a project cycle, and must not gain a
/// spurious one from the new pass.
#[test]
fn single_project_demos_report_only_module_cycles() {
    for demo in ["javascript", "rust"] {
        let report = analyze(demo);
        for finding in report.findings().filter(|f| f.check == CheckId::Cycle) {
            assert_eq!(
                finding.details["scope"], "module",
                "{demo}: {}",
                finding.message
            );
        }
    }
}

#[test]
fn dotnet_demo_enforces_its_layering() {
    let messages = rule_messages(&analyze("dotnet"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`api` must not depend on `data`")),
        "got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`data` must not depend on anything")),
        "got {messages:?}"
    );
}

/// A denylist matches the code, not just the manifest: Shop.Api never declares
/// Serilog, and the rule still fires on the `using`.
#[test]
fn dotnet_demo_denies_a_package_at_the_import() {
    let report = analyze("dotnet");
    let finding = report
        .findings()
        .find(|f| f.check == CheckId::PackageRule && f.message.contains("Serilog"))
        .expect("expected the denylist to fire");
    assert_eq!(finding.details["level"], "imported");
    assert_eq!(
        finding.details["replacement"],
        "Microsoft.Extensions.Logging"
    );
}

#[test]
fn dotnet_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("dotnet"),
        &["cycle", "AutoMapper", "Serilog", "`api`", "`data`"],
    );
}

// --- rulesets ---------------------------------------------------------------

fn rule_messages(report: &Report) -> Vec<String> {
    report
        .findings()
        .filter(|f| matches!(f.check, CheckId::ModuleRule | CheckId::PackageRule))
        .map(|f| f.message.clone())
        .collect()
}

/// A layering rule: the entrypoint must go through the api rather than reaching
/// into storage. Nothing tropism infers could state this.
#[test]
fn go_demo_enforces_its_layering_rule() {
    let messages = rule_messages(&analyze("go"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`cmd` may depend only on `api`")),
        "got {messages:?}"
    );
}

/// A closed-world approved list, the shape regulated teams actually want.
#[test]
fn go_demo_enforces_its_approved_package_list() {
    let messages = rule_messages(&analyze("go"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("golang.org/x/sync") && m.contains("approved list")),
        "got {messages:?}"
    );
}

#[test]
fn javascript_demo_enforces_its_package_policy() {
    let messages = rule_messages(&analyze("javascript"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`left-pad` is not allowed")),
        "{messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("lodash") && m.contains("restricted to")),
        "{messages:?}"
    );
}

/// The motivating case for the whole feature: two surfaces that must not know
/// about each other. Caught at both levels — the manifest declaration and the
/// import — because a rule broken in a manifest is still broken.
#[test]
fn rust_demo_catches_the_independence_violation_at_both_levels() {
    let report = analyze("rust");
    let levels: Vec<&str> = report
        .findings()
        .filter(|f| f.check == CheckId::ModuleRule)
        .filter_map(|f| f.details["level"].as_str())
        .collect();
    assert!(
        levels.contains(&"declared"),
        "Cargo.toml declaration: got {levels:?}"
    );
    assert!(
        levels.contains(&"imported"),
        "main.rs import: got {levels:?}"
    );
}

/// The team's `reason` is the most valuable part of a rule finding: no inferred
/// finding can ever explain why a constraint exists.
#[test]
fn a_rule_finding_carries_the_teams_reason() {
    let report = analyze("rust");
    let finding = report
        .findings()
        .find(|f| f.check == CheckId::ModuleRule)
        .expect("expected an independence violation");
    let notes: Vec<&str> = finding.evidence.iter().map(|e| e.note.as_str()).collect();
    assert!(
        notes.iter().any(|n| n.contains("independent surfaces")),
        "got {notes:?}"
    );
}

#[test]
fn rule_violations_are_high_confidence_and_default_to_error() {
    let report = analyze("rust");
    let finding = report
        .findings()
        .find(|f| f.check == CheckId::ModuleRule)
        .unwrap();
    assert_eq!(finding.confidence, tropism_core::report::Confidence::High);
    assert_eq!(finding.severity, tropism_core::report::Severity::Error);
}

/// Without a ruleset the checks report Unavailable, never a clean pass — the same
/// discipline as a missing lockfile.
#[test]
fn a_project_without_a_ruleset_reports_the_checks_as_unavailable() {
    let providers = tropism_lang::registry();
    let fixtures = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/go-service");
    let report = pipeline::analyze(&fixtures, &providers, &Options::default()).unwrap();
    for check in [CheckId::ModuleRule, CheckId::PackageRule] {
        match &report.projects[0].checks[&check] {
            CheckStatus::Unavailable { reason } => {
                assert!(reason.contains("tropism.toml"), "{reason}")
            }
            other => panic!("{check}: expected unavailable, got {other:?}"),
        }
    }
}

// --- tropism itself ------------------------------------------------------------

/// The dogfood assertion. tropism's own source must produce no findings other than
/// genuine duplicate versions in Cargo.lock — a regression here means a new false
/// positive in the Rust provider.
#[test]
fn tropism_reports_nothing_against_its_own_source() {
    let root = Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let providers = tropism_lang::registry();
    let report = pipeline::analyze(&root, &providers, &Options::default()).unwrap();

    // tropism.toml already excludes the fixtures and demos; the filters below are
    // belt-and-braces in case an exclusion is removed.
    let unexpected: Vec<String> = report
        .projects
        .iter()
        .filter(|p| !p.project.root.as_str().contains("fixtures"))
        .filter(|p| !p.project.root.as_str().starts_with("demo"))
        .flat_map(|p| p.findings.iter().map(move |f| (p.project.root.clone(), f)))
        .filter(|(_, f)| !matches!(f.check, CheckId::VersionConflict | CheckId::DiamondDep))
        .map(|(root, f)| format!("[{root}] {} — {}", f.check, f.message))
        .collect();

    assert!(
        unexpected.is_empty(),
        "tropism found problems in itself:\n{}",
        unexpected.join("\n")
    );
}
