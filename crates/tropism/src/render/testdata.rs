//! Shared fixture for renderer tests.
//!
//! One report exercising every branch a renderer has to handle: a finding with
//! multi-file evidence, a check that ran clean, a check that never ran, a skipped
//! file, and an import exempted by the workspace. Both the text and TUI renderers
//! are tested against it, so the two cannot silently disagree about what a report
//! contains.

use camino::Utf8PathBuf;
use tropism_core::model::{Language, Project};
use tropism_core::report::{
    CheckId, CheckStatus, Confidence, Evidence, ExemptionVia, Finding, ProjectReport, Report,
    Severity, SiblingExemption, SkippedFile,
};

/// The report's scan root. Absolute, because the text renderer reads the fixture
/// sources through it to print the source line under each piece of evidence.
pub fn fixtures() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The scan root a snapshot may contain.
///
/// The TUI header prints the scan root verbatim into a fixed-width buffer, so
/// [`fixtures`] would bake the checkout location into the snapshot — and its
/// length, which decides where the header truncates. Either makes the snapshot
/// pass on exactly one machine. Tests that render the header substitute this.
pub const STABLE_SCAN_ROOT: &str = "crates/tropism/tests/fixtures";

pub fn sample_report() -> Report {
    let project = Project {
        root: Utf8PathBuf::from("cycle"),
        language: Language::Go,
        manifests: vec!["cycle/go.mod".into()],
        lockfile: None,
    };

    let finding = Finding::new(
        CheckId::Cycle,
        &project.root,
        &["cycle/order.go", "cycle/user.go"],
        Severity::Warning,
        Confidence::High,
        "import cycle between 2 packages",
    )
    .with_evidence([
        Evidence::new("cycle/user.go", Some(3), "imports api/order"),
        Evidence::new("cycle/order.go", Some(3), "imports back to api/user"),
    ])
    .with_details(serde_json::json!({
        "members": ["cycle/order.go", "cycle/user.go"]
    }));

    let mut project_report = ProjectReport::new(project);
    project_report.findings.push(finding);
    project_report
        .checks
        .insert(CheckId::Cycle, CheckStatus::Ran { finding_count: 1 });
    project_report
        .checks
        .insert(CheckId::UnusedDep, CheckStatus::Ran { finding_count: 0 });
    project_report.checks.insert(
        CheckId::VersionConflict,
        CheckStatus::unavailable("no lockfile found; resolved tree unknown"),
    );
    // A `missing-dep` that was not reported. It has to appear somewhere, or the
    // project reads as clean when a check deliberately looked away.
    project_report.sibling_exemptions.push(SiblingExemption {
        package: "example.com/shared".to_owned(),
        via: ExemptionVia::WorkspaceSibling,
        provided_by: Some("shared".into()),
        imports: 2,
    });

    let mut report = Report::new(fixtures());
    report.projects.push(project_report);
    report.skipped.push(SkippedFile {
        file: "cycle/generated.go".into(),
        reason: "parse error at line 12".into(),
    });
    report.finalize();
    report
}
