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

// --- Python -----------------------------------------------------------------

/// Python permits a module cycle and fails only at import time, on whichever half
/// happens to be imported second.
#[test]
fn python_demo_finds_the_module_cycle() {
    let report = analyze("python");
    let cycles = messages(&report, CheckId::Cycle);
    assert_eq!(cycles.len(), 1, "got {cycles:?}");
    assert!(cycles[0].contains("2 modules"), "got {cycles:?}");
}

#[test]
fn python_demo_finds_the_hygiene_problems() {
    let report = analyze("python");
    assert!(mentions(&report, CheckId::UnusedDep, "rich"));
    assert!(mentions(&report, CheckId::MissingDep, "httpx"));
}

/// The import→package problem in its sharpest form: `import yaml` is the `PyYAML`
/// distribution, and comparing the two names literally produces a false unused
/// *and* a false missing from one correct line.
#[test]
fn python_demo_does_not_trip_on_its_traps() {
    let report = analyze("python");
    for trap in ["PyYAML", "pyyaml", "yaml", "pytest", "os", "dataclasses"] {
        for check in [CheckId::UnusedDep, CheckId::MissingDep] {
            assert!(!mentions(&report, check, trap), "{check} tripped on {trap}");
        }
    }
}

/// A forked resolution locks one name at two versions, and only one of them is
/// ever installed — a conflict, and never the diamond it superficially resembles.
#[test]
fn python_demo_reports_the_forked_resolution_as_a_conflict_not_a_diamond() {
    let report = analyze("python");
    assert!(mentions(&report, CheckId::VersionConflict, "urllib3"));
    assert_eq!(
        report.projects[0].checks[&CheckId::DiamondDep],
        CheckStatus::Ran { finding_count: 0 },
        "a flat environment installs one copy, so there is nothing to disagree over"
    );
}

#[test]
fn python_demo_enforces_its_rules() {
    let messages = rule_messages(&analyze("python"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`entry` may depend only on `api`")),
        "got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("requests") && m.contains("restricted to")),
        "got {messages:?}"
    );
}

#[test]
fn python_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("python"),
        &["cycle", "rich", "httpx", "urllib3", "`entry`", "requests"],
    );
}

// --- Ruby -------------------------------------------------------------------

/// `require` is idempotent, so a Ruby cycle never raises at load — it resolves to
/// whichever file loaded first, and the other sees a half-defined constant.
#[test]
fn ruby_demo_finds_the_require_cycle() {
    let report = analyze("ruby");
    let cycles = messages(&report, CheckId::Cycle);
    assert_eq!(cycles.len(), 1, "got {cycles:?}");
    assert!(cycles[0].contains("2 modules"), "got {cycles:?}");
}

#[test]
fn ruby_demo_finds_the_hygiene_problems() {
    let report = analyze("ruby");
    assert!(mentions(&report, CheckId::UnusedDep, "awesome_print"));
    assert!(mentions(&report, CheckId::MissingDep, "nokogiri"));
}

/// `faraday/retry` is either a file inside `faraday` or the `faraday-retry` gem,
/// and `shop/order` is this project's own file found through the `lib/` load path.
#[test]
fn ruby_demo_does_not_trip_on_its_traps() {
    let report = analyze("ruby");
    for trap in ["faraday-retry", "shop", "rspec", "json", "pg"] {
        for check in [CheckId::UnusedDep, CheckId::MissingDep] {
            assert!(!mentions(&report, check, trap), "{check} tripped on {trap}");
        }
    }
}

/// Bundler resolves flat and refuses to write a lockfile with two versions of one
/// gem, so both resolved-tree checks run and correctly find nothing.
#[test]
fn ruby_demo_reports_the_resolved_tree_checks_as_clean_not_absent() {
    let report = analyze("ruby");
    for check in [CheckId::VersionConflict, CheckId::DiamondDep] {
        assert_eq!(
            report.projects[0].checks[&check],
            CheckStatus::Ran { finding_count: 0 },
            "{check}: Gemfile.lock is a resolved tree, so the check runs"
        );
    }
}

#[test]
fn ruby_demo_enforces_its_rules() {
    let messages = rule_messages(&analyze("ruby"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`entry` may depend only on `client`")),
        "got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("faraday") && m.contains("restricted to")),
        "got {messages:?}"
    );
}

#[test]
fn ruby_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("ruby"),
        &["cycle", "awesome_print", "nokogiri", "`entry`", "faraday"],
    );
}

// --- Java -------------------------------------------------------------------

/// Two build tools in one demo, so both manifest parsers run end to end.
#[test]
fn java_demo_reads_both_maven_and_gradle() {
    let report = analyze("java");
    let roots: Vec<&str> = report
        .projects
        .iter()
        .map(|p| p.project.root.as_str())
        .collect();
    assert_eq!(roots, vec!["api", "worker"]);
}

/// javac compiles mutually-dependent packages without complaint.
#[test]
fn java_demo_finds_the_package_cycle() {
    let report = analyze("java");
    let cycles = messages(&report, CheckId::Cycle);
    assert_eq!(cycles.len(), 1, "got {cycles:?}");
    assert!(cycles[0].contains("2 modules"), "got {cycles:?}");
}

#[test]
fn java_demo_finds_the_hygiene_problems_in_both_build_tools() {
    let report = analyze("java");
    assert!(
        mentions(&report, CheckId::UnusedDep, "commons-lang3"),
        "pom.xml"
    );
    assert!(
        mentions(&report, CheckId::UnusedDep, "jackson-databind"),
        "build.gradle"
    );
    // guava's coordinate and its package share no segment beyond `com.google`.
    assert!(mentions(
        &report,
        CheckId::MissingDep,
        "com.google.guava:guava"
    ));
}

#[test]
fn java_demo_does_not_trip_on_its_traps() {
    let report = analyze("java");
    for trap in [
        "jackson-bom", // <dependencyManagement> is a version catalogue
        "junit",       // test scope, imported only from src/test/java
        "postgresql",  // runtime scope: on the classpath, never imported
        "java.util",   // platform
        "libs.junit",  // an unresolvable version-catalog reference
    ] {
        for check in [CheckId::UnusedDep, CheckId::MissingDep] {
            assert!(!mentions(&report, check, trap), "{check} tripped on {trap}");
        }
    }
}

/// Maven has no lockfile and `gradle.lockfile` carries no edges, so neither
/// project can answer a resolved-tree question — and says so.
#[test]
fn java_demo_reports_resolved_tree_checks_as_unavailable() {
    let report = analyze("java");
    for project in &report.projects {
        for check in [CheckId::VersionConflict, CheckId::DiamondDep] {
            assert!(
                matches!(project.checks[&check], CheckStatus::Unavailable { .. }),
                "{}: {check}",
                project.project.root
            );
        }
    }
}

/// One rule, two build tools: the Gradle declaration and the Java import are
/// separate findings about the same broken constraint.
#[test]
fn java_demo_catches_the_layering_violation_at_both_levels() {
    let report = analyze("java");
    let levels: Vec<&str> = report
        .findings()
        .filter(|f| f.check == CheckId::ModuleRule)
        .filter_map(|f| f.details["level"].as_str())
        .collect();
    assert!(levels.contains(&"declared"), "build.gradle: got {levels:?}");
    assert!(
        levels.contains(&"imported"),
        "Reconciler.java: got {levels:?}"
    );
}

/// A denylist matches the code: the api never declares guava, and the rule still
/// fires on the import.
#[test]
fn java_demo_denies_a_package_at_the_import() {
    let report = analyze("java");
    let finding = report
        .findings()
        .find(|f| f.check == CheckId::PackageRule)
        .expect("expected the guava rule to fire");
    assert_eq!(finding.details["level"], "imported");
    assert!(finding.message.contains("com.google.guava:guava"));
}

#[test]
fn java_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("java"),
        &[
            "cycle",
            "commons-lang3",
            "jackson-databind",
            "com.google.guava:guava",
            "`worker`",
        ],
    );
}

// --- Swift ------------------------------------------------------------------

#[test]
fn swift_demo_finds_the_hygiene_problems() {
    let report = analyze("swift");
    assert!(mentions(&report, CheckId::UnusedDep, "swift-collections"));
    assert!(mentions(&report, CheckId::MissingDep, "Alamofire"));
}

/// The manifest states the module→package mapping itself, so `import Logging`
/// needs no curated table to reach `swift-log`.
#[test]
fn swift_demo_resolves_a_product_to_its_package_without_guessing() {
    let report = analyze("swift");
    for trap in ["Logging", "swift-log", "Foundation", "XCTest", "ShopCore"] {
        for check in [CheckId::UnusedDep, CheckId::MissingDep] {
            assert!(!mentions(&report, check, trap), "{check} tripped on {trap}");
        }
    }
}

/// SwiftPM rejects a cyclic target dependency outright and files inside a module
/// do not import each other, so a Swift cycle exists only in a package that does
/// not build — the same position as Go.
#[test]
fn swift_demo_has_no_cycle_to_find() {
    let report = analyze("swift");
    assert_eq!(
        report.projects[0].checks[&CheckId::Cycle],
        CheckStatus::Ran { finding_count: 0 }
    );
}

#[test]
fn swift_demo_reports_resolved_tree_checks_as_structurally_unavailable() {
    let report = analyze("swift");
    for check in [CheckId::VersionConflict, CheckId::DiamondDep] {
        match &report.projects[0].checks[&check] {
            CheckStatus::Unavailable { reason } => {
                assert!(reason.contains("Package.resolved"), "{check}: {reason}")
            }
            other => panic!("{check}: expected unavailable, got {other:?}"),
        }
    }
}

#[test]
fn swift_demo_enforces_its_rules() {
    let messages = rule_messages(&analyze("swift"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`cli` may depend only on `core`")),
        "got {messages:?}"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.contains("Logging") && m.contains("restricted to")),
        "got {messages:?}"
    );
}

#[test]
fn swift_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("swift"),
        &["swift-collections", "Alamofire", "`cli`", "Logging"],
    );
}

// --- C++ --------------------------------------------------------------------

/// Include guards make a circular include compile, which is exactly why it
/// survives review: the declarations become order-dependent instead.
#[test]
fn cpp_demo_finds_the_include_cycle() {
    let report = analyze("cpp");
    let cycles = messages(&report, CheckId::Cycle);
    assert_eq!(cycles.len(), 1, "got {cycles:?}");
    assert!(cycles[0].contains("2 modules"), "got {cycles:?}");
}

#[test]
fn cpp_demo_finds_the_hygiene_problems() {
    let report = analyze("cpp");
    assert!(mentions(&report, CheckId::UnusedDep, "nlohmann_json"));
    assert!(mentions(&report, CheckId::MissingDep, "sqlite3"));
}

/// A translation unit including its own header is the most ordinary line in C++,
/// and it is a self-edge only because a header and its source are one module.
#[test]
fn cpp_demo_does_not_trip_on_its_traps() {
    let report = analyze("cpp");
    for trap in ["fmt", "cmake", "gtest", "vector", "sys/stat", "CMakeDeps"] {
        for check in [CheckId::UnusedDep, CheckId::MissingDep] {
            assert!(!mentions(&report, check, trap), "{check} tripped on {trap}");
        }
    }
}

#[test]
fn cpp_demo_reports_resolved_tree_checks_as_structurally_unavailable() {
    let report = analyze("cpp");
    for check in [CheckId::VersionConflict, CheckId::DiamondDep] {
        match &report.projects[0].checks[&check] {
            CheckStatus::Unavailable { reason } => {
                assert!(reason.contains("conan.lock"), "{check}: {reason}")
            }
            other => panic!("{check}: expected unavailable, got {other:?}"),
        }
    }
}

#[test]
fn cpp_demo_enforces_its_rules() {
    let messages = rule_messages(&analyze("cpp"));
    assert!(
        messages
            .iter()
            .any(|m| m.contains("`entry` may depend only on `store`")),
        "got {messages:?}"
    );
    // The store names both its header and its translation unit, so the rule
    // reaches the code rather than reporting an unassigned path.
    assert!(
        messages
            .iter()
            .any(|m| m.contains("spdlog") && m.contains("used in `store`")),
        "got {messages:?}"
    );
}

#[test]
fn cpp_demo_has_no_unexpected_findings() {
    assert_no_unexpected(
        &analyze("cpp"),
        &["cycle", "nlohmann_json", "sqlite3", "`entry`", "spdlog"],
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
