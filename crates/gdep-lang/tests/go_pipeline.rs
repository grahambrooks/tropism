//! End-to-end tests for the Go vertical slice: discovery through to findings.
//!
//! These complement the unit tests. The unit tests prove each stage in isolation;
//! these prove the stages compose, against Go source that a human can read and
//! check by eye.

use camino::{Utf8Path, Utf8PathBuf};
use gdep_core::pipeline::{self, Options};
use gdep_core::report::{CheckId, CheckStatus, Report, Severity};

fn fixture(name: &str) -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn analyze(name: &str) -> Report {
    let providers = gdep_lang::registry();
    pipeline::analyze(&fixture(name), &providers, &Options::default()).expect("analysis failed")
}

fn findings_for(report: &Report, check: CheckId) -> Vec<&gdep_core::report::Finding> {
    report
        .findings()
        .filter(|finding| finding.check == check)
        .collect()
}

fn status(report: &Report, check: CheckId) -> &CheckStatus {
    &report.projects[0].checks[&check]
}

#[test]
fn discovers_the_module_and_runs_every_check() {
    let report = analyze("go-service");
    assert_eq!(report.projects.len(), 1);
    assert_eq!(
        report.projects[0].checks.len(),
        CheckId::ALL.len(),
        "every check must report a status"
    );
}

#[test]
fn finds_the_declared_but_unimported_dependency() {
    let report = analyze("go-service");
    let found = findings_for(&report, CheckId::UnusedDep);
    assert_eq!(
        found.len(),
        1,
        "got: {:?}",
        found.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
    assert!(found[0].message.contains("golang.org/x/sync"));
}

#[test]
fn finds_the_imported_but_undeclared_dependency() {
    let report = analyze("go-service");
    let found = findings_for(&report, CheckId::MissingDep);
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains("github.com/rs/zerolog"));
    assert_eq!(found[0].severity, Severity::Error);
    assert_eq!(found[0].evidence[0].file, "api/api.go");
}

/// `_ "github.com/lib/pq"` is imported for its side effects. It is a real
/// dependency and must not be reported unused.
#[test]
fn a_blank_import_counts_as_usage() {
    let report = analyze("go-service");
    let messages: Vec<&str> = findings_for(&report, CheckId::UnusedDep)
        .iter()
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        !messages.iter().any(|m| m.contains("lib/pq")),
        "got: {messages:?}"
    );
}

/// `// indirect` entries are expected to have no import. Flagging them would put a
/// false positive in nearly every go.mod in existence.
#[test]
fn indirect_requirements_are_not_reported_unused() {
    let report = analyze("go-service");
    let messages: Vec<&str> = findings_for(&report, CheckId::UnusedDep)
        .iter()
        .map(|f| f.message.as_str())
        .collect();
    assert!(
        !messages.iter().any(|m| m.contains("testify")),
        "got: {messages:?}"
    );
}

#[test]
fn stdlib_and_internal_imports_produce_no_findings() {
    let report = analyze("go-service");
    let messages: Vec<&str> = report.findings().map(|f| f.message.as_str()).collect();
    for noise in ["fmt", "strings", "example.com/shop"] {
        assert!(
            !messages.iter().any(|m| m.contains(noise)),
            "{noise} leaked into {messages:?}"
        );
    }
}

/// Go's own compiler rejects import cycles, so this fixture does not build. The
/// analyzer still has to detect it — the check is exercised here precisely because
/// no real Go project can exercise it.
#[test]
fn detects_an_import_cycle() {
    let report = analyze("go-cycle");
    let found = findings_for(&report, CheckId::Cycle);
    assert_eq!(found.len(), 1, "one tangle, one finding");
    assert!(found[0].message.contains("3 modules"));
    assert_eq!(found[0].evidence.len(), 3, "every participant is cited");
}

#[test]
fn a_healthy_module_reports_no_cycles() {
    let report = analyze("go-service");
    assert!(matches!(
        status(&report, CheckId::Cycle),
        CheckStatus::Ran { finding_count: 0 }
    ));
}

/// go.sum is not a resolved tree, and the reason says so rather than claiming no
/// lockfile was found — which would be untrue, since go.sum is right there.
#[test]
fn resolved_tree_checks_report_the_go_specific_reason() {
    let report = analyze("go-service");
    for check in [CheckId::VersionConflict, CheckId::DiamondDep] {
        match status(&report, check) {
            CheckStatus::Unavailable { reason } => {
                assert!(reason.contains("go.sum"), "{check}: {reason}");
                assert!(!reason.contains("no lockfile found"), "{check}: {reason}");
            }
            other => panic!("{check} should be unavailable, got {other:?}"),
        }
    }
}

#[test]
fn findings_carry_verifiable_evidence() {
    let report = analyze("go-service");
    let root = fixture("go-service");
    for finding in report.findings() {
        assert!(
            !finding.evidence.is_empty(),
            "{} has no evidence",
            finding.id
        );
        for evidence in &finding.evidence {
            let path = root.join(&evidence.file);
            assert!(
                path.exists(),
                "{} cites a nonexistent file {path}",
                finding.id
            );

            let line = evidence.line.expect("evidence should cite a line");
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                text.lines().nth(line as usize - 1).is_some(),
                "{} cites {path}:{line}, past the end of the file",
                finding.id
            );
        }
    }
}

/// Principle 5: same input, same bytes out.
#[test]
fn analysis_is_deterministic() {
    let first = analyze("go-service").to_json().unwrap();
    let second = analyze("go-service").to_json().unwrap();
    assert_eq!(first, second);
}

#[test]
fn a_directory_with_no_go_module_yields_no_projects() {
    let providers = gdep_lang::registry();
    let empty = Utf8Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let report = pipeline::analyze(&empty, &providers, &Options::default()).unwrap();
    assert!(report.projects.is_empty());
}

/// Go rejects this with "import cycle not allowed in test", so it is a real
/// defect even though it is not a strongly connected component of the graph.
#[test]
fn detects_a_cycle_that_only_exists_in_a_test_build() {
    let report = analyze("go-test-cycle");
    let found = findings_for(&report, CheckId::Cycle);
    assert_eq!(
        found.len(),
        1,
        "got: {:?}",
        found.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
    assert!(
        found[0].message.contains("test build"),
        "{}",
        found[0].message
    );
    assert_eq!(found[0].details["kind"], "test");
}

/// A module with no go.sum must not be told that "go.sum is not a resolved graph"
/// — that names a file which is not there.
#[test]
fn a_module_without_a_lockfile_gets_the_generic_reason() {
    let providers = gdep_lang::registry();
    let report = pipeline::analyze(&fixture("go-cycle"), &providers, &Options::default()).unwrap();
    match &report.projects[0].checks[&CheckId::VersionConflict] {
        CheckStatus::Unavailable { reason } => {
            assert!(reason.contains("no lockfile"), "{reason}");
            assert!(!reason.contains("go.sum"), "{reason}");
        }
        other => panic!("expected unavailable, got {other:?}"),
    }
}
