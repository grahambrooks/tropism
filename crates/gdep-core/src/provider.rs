//! The `LanguageProvider` abstraction.
//!
//! This trait lives in `gdep-core`, not `gdep-lang`, because analyzers in core need
//! it: the version-conflict analyzer calls `version_ops()` to compare versions
//! correctly per ecosystem. Implementations live in `gdep-lang`.
//!
//! See `design/03-language-providers.md`.

use camino::Utf8Path;

use crate::model::{DeclaredDep, Language, ResolvedDep};

/// An import site found in source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The raw imported path, exactly as written: `github.com/x/y/v2/sub`.
    pub raw: String,
    pub line: u32,
    /// Set when the import cannot affect runtime deps in the usual way
    /// (TypeScript `import type`), so analyzers can weigh it differently.
    pub type_only: bool,
}

/// What an import turned out to refer to.
///
/// `Unresolved` is a first-class outcome, never a silent drop: the share of
/// unresolved imports is the best available proxy for provider completeness, and it
/// caps the confidence of every hygiene finding in that project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    Internal(String),
    External(String),
    Stdlib,
    Unresolved { reason: String },
}

/// Ecosystem-correct version handling. SemVer, PEP 440, Maven, and RubyGems order
/// versions differently; comparing version strings lexically produces wrong
/// conflict findings.
pub trait VersionOps: Send + Sync {
    fn compare(&self, a: &str, b: &str) -> Option<std::cmp::Ordering>;
    fn satisfies(&self, version: &str, requirement: &str) -> Option<bool>;
}

pub trait LanguageProvider: Send + Sync {
    fn language(&self) -> Language;

    /// Filenames whose presence marks a directory as a project root.
    fn manifest_names(&self) -> &'static [&'static str];

    /// Lockfile names, most-preferred first.
    fn lockfile_names(&self) -> &'static [&'static str];

    /// Extensions this provider extracts imports from, without the dot.
    fn source_extensions(&self) -> &'static [&'static str];

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<DeclaredDep>>;

    /// `Ok(None)` when the ecosystem has no lockfile concept or none was found.
    fn parse_lockfile(
        &self,
        path: &Utf8Path,
        text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>>;

    /// Pure: same text in, same imports out. This is what makes caching and
    /// parallelism safe.
    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>>;

    /// Modules needing no declaration.
    fn is_stdlib(&self, module: &str) -> bool;

    fn version_ops(&self) -> &dyn VersionOps;
}
