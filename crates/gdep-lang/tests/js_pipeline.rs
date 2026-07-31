//! End-to-end tests for the JavaScript/TypeScript slice.
//!
//! This is the first ecosystem where all six checks can run, because
//! `package-lock.json` is a genuinely resolved graph. The resolved-tree assertions
//! below are the first time those analyzers have executed against real data.

use camino::Utf8PathBuf;
use gdep_core::pipeline::{self, Options};
use gdep_core::report::{CheckId, CheckStatus, Finding, Report};

fn fixture(name: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn analyze(name: &str) -> Report {
    let providers = gdep_lang::registry();
    pipeline::analyze(&fixture(name), &providers, &Options::default()).expect("analysis failed")
}

fn findings_for(report: &Report, check: CheckId) -> Vec<&Finding> {
    report
        .findings()
        .filter(|finding| finding.check == check)
        .collect()
}

#[test]
fn every_check_runs_when_a_real_lockfile_is_present() {
    let report = analyze("js-app");
    let unavailable: Vec<CheckId> = report
        .projects
        .iter()
        .flat_map(|p| p.checks.iter())
        .filter(|(_, status)| matches!(status, CheckStatus::Unavailable { .. }))
        .map(|(check, _)| *check)
        .collect();

    // Only dependency-bloat, which is deferred by design rather than blocked.
    assert_eq!(
        unavailable,
        vec![CheckId::DependencyBloat],
        "got {unavailable:?}"
    );
}

/// File-level, and mutual — the common and genuinely painful shape in JS/TS, and
/// one that a directory-level graph like Go's would miss entirely.
#[test]
fn detects_a_file_level_import_cycle() {
    let report = analyze("js-app");
    let found = findings_for(&report, CheckId::Cycle);
    assert_eq!(found.len(), 1);
    let members = found[0].details["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert!(found[0].evidence.len() == 2, "both directions cited");
}

#[test]
fn finds_declared_but_unimported_packages() {
    let report = analyze("js-app");
    let messages: Vec<&str> = findings_for(&report, CheckId::UnusedDep)
        .iter()
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("left-pad")),
        "got {messages:?}"
    );
}

#[test]
fn finds_the_imported_but_undeclared_package() {
    let report = analyze("js-app");
    let found = findings_for(&report, CheckId::MissingDep);
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains("chalk"));
}

#[test]
fn node_builtins_and_relative_imports_are_never_reported() {
    let report = analyze("js-app");
    let messages: Vec<&str> = report.findings().map(|f| f.message.as_str()).collect();
    for noise in ["node:fs", "./utils", "./format"] {
        assert!(
            !messages.iter().any(|m| m.contains(noise)),
            "{noise} leaked into {messages:?}"
        );
    }
}

/// The first check in the project's history to run against a real resolved tree.
#[test]
fn detects_duplicate_versions_in_the_resolved_tree() {
    let report = analyze("js-app");
    let found = findings_for(&report, CheckId::VersionConflict);
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains("ms"));
    let versions = found[0].details["versions"].as_array().unwrap();
    assert_eq!(versions.len(), 2);
}

/// Counting dependents per copy finds nothing here — npm gives each duplicated
/// copy exactly one dependent. Counting per package name is what makes the
/// diamond visible.
#[test]
fn attributes_a_diamond_to_the_dependents_that_disagreed() {
    let report = analyze("js-app");
    let found = findings_for(&report, CheckId::DiamondDep);
    assert_eq!(found.len(), 1);
    let dependents = found[0].details["dependents"].as_object().unwrap();
    assert_eq!(dependents["express"], "2.1.3");
    assert_eq!(dependents["vitest"], "2.0.0");
}

#[test]
fn findings_carry_verifiable_evidence() {
    let report = analyze("js-app");
    let root = fixture("js-app");
    for finding in report.findings() {
        assert!(
            !finding.evidence.is_empty(),
            "{} has no evidence",
            finding.id
        );
        for evidence in &finding.evidence {
            assert!(
                root.join(&evidence.file).exists(),
                "{} cites a nonexistent file {}",
                finding.id,
                evidence.file
            );
        }
    }
}

#[test]
fn analysis_is_deterministic() {
    assert_eq!(
        analyze("js-app").to_json().unwrap(),
        analyze("js-app").to_json().unwrap()
    );
}
