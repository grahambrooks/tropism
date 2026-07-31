//! Shared fixture for renderer tests.
//!
//! One report exercising every branch a renderer has to handle: a finding with
//! multi-file evidence, a check that ran clean, a check that never ran, and a
//! skipped file. Both the text and TUI renderers are tested against it, so the two
//! cannot silently disagree about what a report contains.

use camino::Utf8PathBuf;
use gdep_core::model::{Language, Project};
use gdep_core::report::{
    CheckId, CheckStatus, Confidence, Evidence, Finding, ProjectReport, Report, Severity,
    SkippedFile,
};

pub fn fixtures() -> Utf8PathBuf {
    Utf8PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

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

    let mut report = Report::new(fixtures());
    report.projects.push(project_report);
    report.skipped.push(SkippedFile {
        file: "cycle/generated.go".into(),
        reason: "parse error at line 12".into(),
    });
    report.finalize();
    report
}
