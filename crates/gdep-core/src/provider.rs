//! The `LanguageProvider` abstraction.
//!
//! This trait lives in `gdep-core`, not `gdep-lang`, because analyzers in core need
//! it: the version-conflict analyzer calls `version_ops()` to compare versions
//! correctly per ecosystem. Implementations live in `gdep-lang`.
//!
//! See `design/03-language-providers.md`.

use camino::{Utf8Path, Utf8PathBuf};

use crate::graph::ModuleId;
use crate::model::{DeclaredDep, Language, Manifest, Project, ResolvedDep};

/// What a provider needs to know about the project to resolve an import.
pub struct ProjectContext<'a> {
    pub project: &'a Project,
    /// The package's own identity from its manifest — Go's module path, npm's
    /// `name`. Required to distinguish internal imports from external ones.
    pub package_name: Option<&'a str>,
    pub declared: &'a [DeclaredDep],
    /// Names of packages published by *other* projects in the same scan.
    ///
    /// In a monorepo, `@tanstack/query-core` is imported by its siblings and
    /// declared only at the workspace root. Without this, every such import is
    /// reported as an undeclared dependency — 101 of them in TanStack Query.
    /// Resolves open question 1 in `design/07-open-questions.md`.
    pub sibling_packages: &'a [String],
    /// Every source file in the project, relative to the scan root.
    ///
    /// Needed to resolve extensionless relative imports: JavaScript's `./utils`
    /// may mean `utils.ts`, `utils.tsx`, or `utils/index.ts`, and only the file
    /// list distinguishes them. Go never needed this; the second language did.
    pub source_files: &'a [Utf8PathBuf],
}

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

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest>;

    /// `Ok(None)` when the ecosystem has no lockfile concept or none was found.
    fn parse_lockfile(
        &self,
        path: &Utf8Path,
        text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>>;

    /// Why this ecosystem cannot supply a resolved tree, when that is structural
    /// rather than a missing file.
    ///
    /// Go is the motivating case: `go.sum` exists and looks like a lockfile, but it
    /// records hashes for the whole module graph rather than the resolved selection,
    /// so it cannot answer version-conflict or diamond questions. Saying so beats
    /// the generic "no lockfile found", which would be simply untrue.
    fn resolved_tree_note(&self) -> Option<&'static str> {
        None
    }

    /// Pure: same text in, same imports out. This is what makes caching and
    /// parallelism safe.
    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>>;

    /// Which module a source file belongs to, given the directory-derived default.
    ///
    /// Most languages map a file to its directory, which is the default. Go needs an
    /// override and it is not cosmetic: a file in `promql/` declaring
    /// `package promql_test` is a *different* package from `package promql`, and Go
    /// deliberately allows it to import packages that import `promql`. Collapsing
    /// the two onto one node invents import cycles that do not exist.
    ///
    /// Java will need this too — its package comes from the `package` declaration
    /// rather than the directory.
    fn module_id_for_file(&self, _path: &Utf8Path, _text: &str, default_id: &str) -> ModuleId {
        ModuleId::module(default_id)
    }

    /// Classifies one import, given the file it appears in.
    ///
    /// `from` is required because a relative specifier means nothing without it —
    /// `./b` in `src/a.ts` and in `lib/a.ts` are different modules. Go's import
    /// paths are absolute so it ignores this, which is why the first version of
    /// this trait lacked the parameter.
    ///
    /// See `design/03-language-providers.md` — resolution failure must surface as
    /// [`ImportTarget::Unresolved`], never a silent drop.
    fn resolve_import(
        &self,
        import: &Import,
        from: &Utf8Path,
        ctx: &ProjectContext<'_>,
    ) -> ImportTarget;

    /// Modules needing no declaration.
    fn is_stdlib(&self, module: &str) -> bool;

    fn version_ops(&self) -> &dyn VersionOps;
}
