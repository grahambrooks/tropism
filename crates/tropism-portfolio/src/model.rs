//! The portfolio report: a public, versioned contract.
//!
//! Other tools read this — ggh's graph view, the `portfolio-impact` skill, and
//! sustain in place of its own discovery — so it follows the rule
//! `tropism-core::report` follows: [`SCHEMA_VERSION`] increments on any breaking
//! change, and consumers must ignore fields they do not know.
//!
//! It is a separate contract from `tropism analyze`'s, not an extension of it,
//! because its unit is a repository rather than a project. Findings keep the
//! same shape — `id`, `check`, `severity`, `confidence`, `message`, `evidence`,
//! `details` — with the core's own `Severity`, `Confidence`, `Evidence` and
//! `CheckStatus` types, so a consumer of one already reads the other.
//!
//! See `design/20-portfolio.md`.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use serde::Serialize;
use tropism_core::report::{CheckStatus, Confidence, Evidence, Severity};

pub const SCHEMA: &str = "tropism-portfolio";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize)]
pub struct PortfolioReport {
    pub schema: &'static str,
    pub schema_version: u32,
    pub tool: Tool,
    pub owner: String,
    /// The roots as given on the command line.
    pub roots: Vec<Utf8PathBuf>,
    /// Where tags were read from: `local` (the checkout's refs, as last fetched)
    /// or `remote` (`--fetch`, `git ls-remote`).
    pub tags_from: TagSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rules: Option<Utf8PathBuf>,
    pub repos: Vec<Repo>,
    pub libraries: Vec<Library>,
    pub edges: Vec<Edge>,
    pub checks: BTreeMap<PortfolioCheck, CheckStatus>,
    pub findings: Vec<PortfolioFinding>,
    /// Exclusions applied, with how much each removed. A silent blind spot is the
    /// failure the rest of tropism is built to avoid.
    pub exclusions: Vec<Exclusion>,
    /// Files that could not be read or parsed, and other things that stopped a
    /// fact from being established. Never empty-means-clean: a file listed here
    /// contributed nothing.
    pub notes: Vec<Note>,
    /// Structural limits of the method, stated in every report.
    pub limitations: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: &'static str,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TagSource {
    Local,
    Remote,
}

#[derive(Debug, Clone, Serialize)]
pub struct Repo {
    /// The repository's name on its remote, or the directory name when it has
    /// none. Unique within a report; edges refer to repos by it.
    pub name: String,
    pub path: Utf8PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Whether the remote belongs to `owner`.
    pub own: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Packages and modules this repository publishes.
    pub packages: Vec<PackageRef>,
    /// Binaries this repository builds.
    pub binaries: Vec<String>,
    /// Other checkouts of the same remote, ignored in favour of this one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duplicate_checkouts: Vec<Utf8PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PackageRef {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub manifest: Utf8PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ecosystem {
    Cargo,
    Go,
    Npm,
}

#[derive(Debug, Clone, Serialize)]
pub struct Library {
    /// The repository's name; for an unavailable library, its remote's name.
    pub repo: String,
    pub remote: String,
    pub status: LibraryStatus,
    pub packages: Vec<PackageRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_tag: Option<String>,
    pub release: Release,
    /// Repositories with at least one edge to this one.
    pub users: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LibraryStatus {
    Available,
    /// Linked to but not checked out under any root, so it contributes nothing:
    /// not its packages, not its tags, not its own links.
    Unavailable,
}

/// What the library's default branch has that its newest tag does not.
#[derive(Debug, Clone, Serialize)]
pub struct Release {
    pub status: ReleaseStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// `sha subject` of each commit its users would get from a release.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseStatus {
    /// Nothing users would get: no commits, or only lock-only and release-bot ones.
    Current,
    Pending,
    NoTag,
    /// The newest tag or the default branch is not in this checkout's history.
    NotComparable,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: LinkKind,
    /// The package (Cargo, npm), module path (Go) or binary name on the library
    /// side.
    pub package: String,
    pub via: Via,
    /// For a transitive edge, the packages in the user's lockfile that pull it in.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub through: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<Pin>,
    /// What each lockfile resolved, one entry per copy.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolved: Vec<Resolved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest: Option<String>,
    /// Stable tags newer than what the user is on. Absent when it cannot be
    /// established, with `behind_note` saying why — never a silent zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_by: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_note: Option<String>,
    pub confidence: Confidence,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkKind {
    Cargo,
    Go,
    Npm,
    /// An installed binary, found by text matching rather than import
    /// resolution. Always medium confidence; never gates by default.
    Binary,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::Go => "go",
            Self::Npm => "npm",
            Self::Binary => "binary",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Cargo, Self::Go, Self::Npm, Self::Binary]
            .into_iter()
            .find(|kind| kind.as_str() == text)
    }
}

impl From<Ecosystem> for LinkKind {
    fn from(ecosystem: Ecosystem) -> Self {
        match ecosystem {
            Ecosystem::Cargo => Self::Cargo,
            Ecosystem::Go => Self::Go,
            Ecosystem::Npm => Self::Npm,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    /// Declared in one of the user's manifests.
    Direct,
    /// Only in the user's lockfile, pulled in by another dependency.
    Transitive,
    /// Go's `// indirect`: in `go.mod`, but nothing imports it directly.
    Indirect,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Pin {
    pub kind: PinKind,
    pub value: String,
}

impl Pin {
    pub fn new(kind: PinKind, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }
}

impl std::fmt::Display for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            PinKind::Tag | PinKind::Version => f.write_str(&self.value),
            PinKind::DefaultBranch => f.write_str("default branch"),
            kind => write!(f, "{} {}", kind.as_str(), self.value),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PinKind {
    Tag,
    Rev,
    Branch,
    /// No ref given: whatever the default branch was when the lockfile was made.
    DefaultBranch,
    /// An exact version (Go, an npm lockfile).
    Version,
    /// A requirement that admits several versions (`^1.2`).
    Range,
    /// A local directory: `path = "../lib"`, `replace … => ../lib`.
    Path,
}

impl PinKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tag => "tag",
            Self::Rev => "rev",
            Self::Branch => "branch",
            Self::DefaultBranch => "default-branch",
            Self::Version => "version",
            Self::Range => "range",
            Self::Path => "path",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Resolved {
    pub lockfile: Utf8PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<Pin>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PortfolioCheck {
    /// A library's default branch has commits its users would get that no tag
    /// has released.
    ReleasePending,
    /// A user is on an older tag than the library's newest.
    TagsBehind,
    /// One lockfile resolves more than one copy of an own library.
    DuplicateCopy,
    /// A portfolio rule names a library or repository that does not exist.
    StaleRule,
}

impl PortfolioCheck {
    pub const ALL: [Self; 4] = [
        Self::ReleasePending,
        Self::TagsBehind,
        Self::DuplicateCopy,
        Self::StaleRule,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReleasePending => "release-pending",
            Self::TagsBehind => "tags-behind",
            Self::DuplicateCopy => "duplicate-copy",
            Self::StaleRule => "stale-rule",
        }
    }
}

impl std::fmt::Display for PortfolioCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PortfolioFinding {
    /// `check:repo:hash`, stable across runs, machines and orderings.
    pub id: String,
    pub check: PortfolioCheck,
    pub severity: Severity,
    pub confidence: Confidence,
    /// The repository the finding is about: the user for `tags-behind` and
    /// `duplicate-copy`, the library for `release-pending`.
    pub repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library: Option<String>,
    /// The portfolio rule that set the severity, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub message: String,
    pub evidence: Vec<Evidence>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

impl PortfolioFinding {
    pub fn id(check: PortfolioCheck, repo: &str, key_parts: &[&str]) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(check.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(repo.as_bytes());
        for part in key_parts {
            hasher.update(b"\0");
            hasher.update(part.as_bytes());
        }
        format!("{check}:{repo}:{}", &hasher.finalize().to_hex()[..8])
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Exclusion {
    pub pattern: String,
    /// Where the pattern came from: a repository's `tropism.toml`, or the
    /// portfolio rules file.
    pub source: Utf8PathBuf,
    pub matched: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Note {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<Utf8PathBuf>,
    pub message: String,
}

impl PortfolioReport {
    pub fn max_severity(&self) -> Option<Severity> {
        self.findings.iter().map(|f| f.severity).max()
    }

    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}
