//! The output contract. This is a public API: `SCHEMA_VERSION` increments on any
//! breaking change, and consumers must ignore unknown fields.
//!
//! See `design/05-interfaces.md`.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::model::Project;

/// The report contract's version. **Not** `tropism.toml`'s `schema_version`, which
/// versions the ruleset file format and is unrelated.
///
/// `2` — D40. Three languages were spelled `java-script`, `type-script` and
/// `c-sharp` in JSON and `javascript`, `typescript`, `csharp` everywhere else; the
/// contract now uses the second spelling throughout. That changes a published
/// value, so it is a breaking change by the rule at the top of this file, and the
/// rule is applied even though nothing has been published to crates.io yet — a
/// version that is not incremented when the contract changes tells a consumer
/// nothing, which is the failure this project refuses everywhere else.
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckId {
    Cycle,
    UnusedDep,
    MissingDep,
    VersionConflict,
    DiamondDep,
    DependencyBloat,
    /// Team-authored architecture rules — see `design/11-dependency-rules.md`.
    ModuleRule,
    PackageRule,
}

impl CheckId {
    /// Checks produced by the analyzers, from a project's own graph and manifests.
    pub const ANALYSIS: [CheckId; 6] = [
        CheckId::Cycle,
        CheckId::UnusedDep,
        CheckId::MissingDep,
        CheckId::VersionConflict,
        CheckId::DiamondDep,
        CheckId::DependencyBloat,
    ];

    /// Checks produced by the ruleset, which is evaluated repo-wide after the
    /// per-project pass and so cannot come from an analyzer.
    pub const RULES: [CheckId; 2] = [CheckId::ModuleRule, CheckId::PackageRule];

    pub const ALL: [CheckId; 8] = [
        CheckId::Cycle,
        CheckId::UnusedDep,
        CheckId::MissingDep,
        CheckId::VersionConflict,
        CheckId::DiamondDep,
        CheckId::DependencyBloat,
        CheckId::ModuleRule,
        CheckId::PackageRule,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cycle => "cycle",
            Self::UnusedDep => "unused-dep",
            Self::MissingDep => "missing-dep",
            Self::VersionConflict => "version-conflict",
            Self::DiamondDep => "diamond-dep",
            Self::DependencyBloat => "dependency-bloat",
            Self::ModuleRule => "module-rule",
            Self::PackageRule => "package-rule",
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
///
/// A project at the scan root has an empty path; it displays as `.` so IDs never
/// contain an empty component.
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
    let label = if project.as_str().is_empty() {
        "."
    } else {
        project.as_str()
    };
    format!("{check}:{label}:{}", &hex[..8])
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
    /// Source files this project contributed to the analysis.
    ///
    /// Additive, and consumers may ignore it. It exists so `tropism check` can say
    /// how much it looked at: "checked 6 file(s)" and "checked 1,204 file(s)" are
    /// different claims, and a run that examined a handful must not read like one
    /// that examined the repository.
    #[serde(default)]
    pub source_file_count: usize,
    /// Imports `missing-dep` skipped because the workspace supplies them.
    ///
    /// Additive; consumers may ignore it. It exists so the exemption is auditable:
    /// before it, an undeclared cross-package import inside a workspace produced
    /// nothing at all in the output, which reads exactly like a clean project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sibling_exemptions: Vec<SiblingExemption>,
    /// How much of this project's source resolved. `None` for a rules-only run,
    /// which does not resolve imports at all — absent rather than zero, so nobody
    /// reads "did not look" as "understood nothing".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
}

impl ProjectReport {
    pub fn new(project: Project) -> Self {
        Self {
            project,
            checks: BTreeMap::new(),
            findings: Vec::new(),
            source_file_count: 0,
            sibling_exemptions: Vec::new(),
            resolution: None,
        }
    }

    /// Sorts findings into the canonical order. Call before serializing.
    pub fn finalize(&mut self) {
        self.findings
            .sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
        self.sibling_exemptions
            .sort_by(|a, b| a.package.cmp(&b.package));
    }

    pub fn max_severity(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }
}

/// Why an import needed no declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExemptionVia {
    /// Another project in the same workspace publishes this name.
    WorkspaceSibling,
    /// An ancestor project declares it, and module resolution walks upward.
    HoistedFromAncestor,
}

impl ExemptionVia {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceSibling => "workspace sibling",
            Self::HoistedFromAncestor => "hoisted from ancestor",
        }
    }
}

/// An import that `missing-dep` did not report because the workspace supplies it.
///
/// Disclosed for the same reason [`Exclusion`] is: this is a deliberate blind spot,
/// and a silent blind spot is the failure mode `CheckStatus` exists to prevent. It
/// is also the honest form of a judgement call — an undeclared sibling import
/// resolves through hoisting today and breaks the moment the package is published
/// or built in isolation, so a reader deserves to know the exemption happened even
/// while tropism declines to call it an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiblingExemption {
    pub package: String,
    pub via: ExemptionVia,
    /// The project root that supplies the name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provided_by: Option<Utf8PathBuf>,
    /// How many import sites this exemption covered.
    pub imports: usize,
}

/// How much of a project's source tropism actually understood.
///
/// **The best available proxy for provider completeness**, and already the thing
/// that caps every hygiene finding's confidence — `unused-dep` and `missing-dep`
/// drop to `Low` below 90%. It was computed and then thrown away, so a reader could
/// see a Low-confidence finding without being able to see *why*, and an evaluation
/// could not measure the number its own conclusions depend on.
///
/// `reasons` is the actionable part: it names the next provider gap directly, in
/// the project's own vocabulary. An `Unresolved` import is never a silent drop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    pub imports: usize,
    pub resolved: usize,
    pub unresolved: usize,
    /// `resolved / imports`, or 1.0 for a project with no imports at all. **This is
    /// the number that caps hygiene confidence.**
    pub rate: f64,
    /// Import *statements* only, excluding bare path references.
    pub statements: usize,
    /// Resolution over statements alone.
    ///
    /// The honest measure of provider completeness, and usually very different from
    /// [`Self::rate`]. Rust leaves an unrecognised path root `Unresolved` by
    /// design — `Vec`, `String` and every local type land there — so `rate` on a
    /// Rust project largely counts a deliberate decision, while this counts imports
    /// tropism was asked to resolve and could not.
    pub statement_rate: f64,
    /// The most common reasons an import did not resolve, most frequent first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<UnresolvedReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedReason {
    pub reason: String,
    pub count: usize,
}

/// A path tropism suggests excluding, and what excluding it would cost.
///
/// A first run on a real repository has no `tropism.toml`, so it reports on every
/// manifest it finds — including the fixture packages under `tests/` that exist to
/// be broken. Measured across the 24-repository evaluation corpus, **19% of hygiene
/// findings sat inside a test fixture, example or sample**, and 725 of deno's 797
/// "projects" were fixtures. That is not a discovery bug, and it is not something a
/// provider can fix: those really are manifests.
///
/// So tropism offers the block rather than applying it. `projects` and `findings`
/// are on the suggestion because an exclusion is a **deliberate blind spot** — the
/// user has to be told what it hides before adopting it, which is the same reason
/// [`Exclusion`] carries a match count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedExclusion {
    /// A glob ready to paste into `tropism.toml`'s `exclude`.
    pub pattern: String,
    /// Projects it would remove from the analysis.
    pub projects: usize,
    /// Findings it would remove with them.
    pub findings: usize,
}

/// A path deliberately kept out of the analysis.
///
/// Disclosed for the same reason `CheckStatus` exists: a repository that excluded
/// half of itself must not look like one that was fully analyzed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exclusion {
    pub pattern: String,
    /// How many paths this pattern kept out. Zero means the pattern is stale.
    pub matched: usize,
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
    pub tropism_version: String,
    pub scan_root: Utf8PathBuf,
    pub projects: Vec<ProjectReport>,
    pub skipped: Vec<SkippedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded: Vec<Exclusion>,
    /// Paths worth excluding, offered rather than applied. Empty when the ruleset
    /// already excludes them, since they are filtered before discovery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_excludes: Vec<SuggestedExclusion>,
}

impl Report {
    pub fn new(scan_root: impl Into<Utf8PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            tropism_version: env!("CARGO_PKG_VERSION").to_string(),
            scan_root: scan_root.into(),
            projects: Vec::new(),
            skipped: Vec::new(),
            excluded: Vec::new(),
            suggested_excludes: Vec::new(),
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
