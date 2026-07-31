//! The output contract. This is a public API: `SCHEMA_VERSION` increments on any
//! breaking change, and consumers must ignore unknown fields.
//!
//! See `design/05-interfaces.md`.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::model::Project;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckId {
    Cycle,
    UnusedDep,
    MissingDep,
    VersionConflict,
    DiamondDep,
    DependencyBloat,
}

impl CheckId {
    pub const ALL: [CheckId; 6] = [
        CheckId::Cycle,
        CheckId::UnusedDep,
        CheckId::MissingDep,
        CheckId::VersionConflict,
        CheckId::DiamondDep,
        CheckId::DependencyBloat,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cycle => "cycle",
            Self::UnusedDep => "unused-dep",
            Self::MissingDep => "missing-dep",
            Self::VersionConflict => "version-conflict",
            Self::DiamondDep => "diamond-dep",
            Self::DependencyBloat => "dependency-bloat",
        }
    }
}

impl std::fmt::Display for CheckId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            other => Err(format!("unknown severity `{other}`")),
        }
    }
}

/// How much the *analysis method* is trusted, independent of severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

/// What supports a finding. Mandatory: a finding an agent cannot verify is one it
/// must take on faith.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub file: Utf8PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub note: String,
}

impl Evidence {
    pub fn new(file: impl Into<Utf8PathBuf>, line: Option<u32>, note: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line,
            note: note.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub check: CheckId,
    pub severity: Severity,
    pub confidence: Confidence,
    pub message: String,
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

impl Finding {
    /// Builds a finding with a content-derived ID.
    ///
    /// `key_parts` must identify the finding independently of iteration order — for a
    /// cycle, the *sorted* members. Anything positional makes IDs unstable between
    /// runs and breaks suppression.
    pub fn new(
        check: CheckId,
        project: &Utf8PathBuf,
        key_parts: &[&str],
        severity: Severity,
        confidence: Confidence,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: finding_id(check, project, key_parts),
            check,
            severity,
            confidence,
            message: message.into(),
            evidence: Vec::new(),
            details: serde_json::Value::Null,
        }
    }

    #[must_use]
    pub fn with_evidence(mut self, evidence: impl IntoIterator<Item = Evidence>) -> Self {
        self.evidence.extend(evidence);
        self
    }

    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    /// Stable sort key. Ordering must not depend on discovery order.
    pub fn sort_key(&self) -> (CheckId, &str) {
        (self.check, self.id.as_str())
    }
}

/// `check:project:hash` — stable across runs, machines, and orderings.
pub fn finding_id(check: CheckId, project: &Utf8PathBuf, key_parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(check.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(project.as_str().as_bytes());
    for part in key_parts {
        hasher.update(b"\0");
        hasher.update(part.as_bytes());
    }
    let hex = hasher.finalize().to_hex();
    format!("{check}:{project}:{}", &hex[..8])
}

/// Whether a check produced an answer — and if not, why not.
///
/// A consumer seeing zero findings must be able to tell "clean" from "never ran".
/// That distinction is the entire reason this type exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CheckStatus {
    Ran { finding_count: usize },
    Unavailable { reason: String },
    Failed { error: String },
}

impl CheckStatus {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self::Unavailable {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectReport {
    #[serde(flatten)]
    pub project: Project,
    pub checks: BTreeMap<CheckId, CheckStatus>,
    pub findings: Vec<Finding>,
}

impl ProjectReport {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            checks: BTreeMap::new(),
            findings: Vec::new(),
        }
    }

    /// Sorts findings into the canonical order. Call before serializing.
    pub fn finalize(&mut self) {
        self.findings
            .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    }

    pub fn max_severity(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }
}

/// A file that could not be parsed. Recorded so silence is never mistaken for
/// cleanliness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedFile {
    pub file: Utf8PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub gdep_version: String,
    pub scan_root: Utf8PathBuf,
    pub projects: Vec<ProjectReport>,
    pub skipped: Vec<SkippedFile>,
}

impl Report {
    pub fn new(scan_root: impl Into<Utf8PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            gdep_version: env!("CARGO_PKG_VERSION").to_string(),
            scan_root: scan_root.into(),
            projects: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// Sorts everything into canonical order. Same input must produce same bytes.
    pub fn finalize(&mut self) {
        for project in &mut self.projects {
            project.finalize();
        }
        self.projects
            .sort_by(|a, b| a.project.root.cmp(&b.project.root));
        self.skipped.sort_by(|a, b| a.file.cmp(&b.file));
    }

    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.projects.iter().flat_map(|p| p.findings.iter())
    }

    /// The machine-readable form, used by `--format json` *and* by the MCP server.
    ///
    /// Both surfaces serialize through here rather than each calling serde
    /// themselves, so an agent reading MCP output and a human reading piped CLI
    /// output are looking at exactly the same contract.
    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn max_severity(&self) -> Option<Severity> {
        self.projects
            .iter()
            .filter_map(ProjectReport::max_severity)
            .max()
    }

    /// Checks that did not run, as `(project, check, reason)`. Surfaced prominently
    /// by every renderer — an empty finding list means nothing without this.
    pub fn unavailable(&self) -> impl Iterator<Item = (&Utf8PathBuf, CheckId, &str)> {
        self.projects.iter().flat_map(|p| {
            p.checks
                .iter()
                .filter_map(move |(check, status)| match status {
                    CheckStatus::Unavailable { reason } => {
                        Some((&p.project.root, *check, reason.as_str()))
                    }
                    _ => None,
                })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Utf8PathBuf {
        Utf8PathBuf::from("backend")
    }

    #[test]
    fn finding_id_is_stable_across_calls() {
        let a = finding_id(CheckId::Cycle, &project(), &["a.rs", "b.rs"]);
        let b = finding_id(CheckId::Cycle, &project(), &["a.rs", "b.rs"]);
        assert_eq!(a, b);
        assert!(a.starts_with("cycle:backend:"));
    }

    #[test]
    fn finding_id_distinguishes_check_project_and_content() {
        let base = finding_id(CheckId::Cycle, &project(), &["a.rs"]);
        assert_ne!(base, finding_id(CheckId::UnusedDep, &project(), &["a.rs"]));
        assert_ne!(
            base,
            finding_id(CheckId::Cycle, &Utf8PathBuf::from("web"), &["a.rs"])
        );
        assert_ne!(base, finding_id(CheckId::Cycle, &project(), &["b.rs"]));
    }

    /// Key parts are order-sensitive by design, so callers must sort. This test
    /// documents that contract rather than asserting it away.
    #[test]
    fn finding_id_depends_on_key_part_order() {
        let forward = finding_id(CheckId::Cycle, &project(), &["a.rs", "b.rs"]);
        let reverse = finding_id(CheckId::Cycle, &project(), &["b.rs", "a.rs"]);
        assert_ne!(
            forward, reverse,
            "callers must sort key parts before building an ID"
        );
    }

    #[test]
    fn severity_orders_info_below_error() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn unavailable_checks_are_reported_separately_from_findings() {
        let mut report = Report::new(".");
        let mut pr = ProjectReport::new(Project {
            root: project(),
            language: crate::model::Language::Go,
            manifests: vec!["backend/go.mod".into()],
            lockfile: None,
        });
        pr.checks.insert(
            CheckId::VersionConflict,
            CheckStatus::unavailable("no lockfile"),
        );
        report.projects.push(pr);

        assert_eq!(report.findings().count(), 0);
        let unavailable: Vec<_> = report.unavailable().collect();
        assert_eq!(unavailable.len(), 1, "zero findings must not read as clean");
        assert_eq!(unavailable[0].1, CheckId::VersionConflict);
    }
}
