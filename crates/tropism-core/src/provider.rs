//! The `LanguageProvider` abstraction.
//!
//! This trait lives in `tropism-core`, not `tropism-lang`, because analyzers in core need
//! it: the version-conflict analyzer calls `version_ops()` to compare versions
//! correctly per ecosystem. Implementations live in `tropism-lang`.
//!
//! See `design/03-language-providers.md`.

use std::collections::BTreeSet;
use std::sync::OnceLock;

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
    /// Names of packages published by other projects *in the same workspace*, plus
    /// dependencies an ancestor project hoists.
    ///
    /// In a monorepo, `@tanstack/query-core` is imported by its siblings and
    /// declared only at the workspace root. Without this, every such import is
    /// reported as an undeclared dependency — 101 of them in TanStack Query.
    ///
    /// The membership question — *which* projects count as siblings — is
    /// [`crate::workspace`]'s, and it is not "all of them": the set is bounded by
    /// the workspace and always by language, so a Rust crate can never make a
    /// JavaScript import look declared. See `design/07-open-questions.md`,
    /// question 1.
    pub sibling_packages: &'a [String],
    /// Modules this project defines, known before any import is resolved.
    ///
    /// C# needs it: a `using` names a namespace, and the only way to tell an
    /// internal namespace from a package is to know which ones the project
    /// declares.
    ///
    /// Contains *every* module kind, including integration tests and benchmarks. A
    /// provider wanting a filtered view derives its own — see
    /// [`Self::local_modules`], and D39 for what it cost when that derivation was
    /// not memoized. This comment used to claim Rust used this field "to avoid
    /// recomputing its module set per import"; it did not, and nothing checked.
    pub known_modules: &'a BTreeSet<String>,
    /// Every source file in the project, relative to the scan root.
    ///
    /// Needed to resolve extensionless relative imports: JavaScript's `./utils`
    /// may mean `utils.ts`, `utils.tsx`, or `utils/index.ts`, and only the file
    /// list distinguishes them. Go never needed this; the second language did.
    pub source_files: &'a [Utf8PathBuf],
    /// A provider-defined module set, derived from this context at most once.
    ///
    /// [`Self::known_modules`] is the pipeline's view and contains every module
    /// kind. A provider needing a *filtered* one — Rust excludes integration tests
    /// and benchmarks, which are separate crates that may freely use the library —
    /// has to derive it from [`Self::source_files`], and deriving it per import is
    /// O(files) per import: quadratic over a project, and invisible until a
    /// repository gets large.
    ///
    /// Measured before this existed: 2,000 Rust files took 8.6 s and 3,000 took
    /// 19.6 s, against 0.56 s for 500. Parallelising extraction (D4) barely moved
    /// it, because the cost was here and not in the parse.
    pub local_modules: OnceLock<BTreeSet<String>>,
    /// Build-tool path aliases in effect for this project.
    ///
    /// `@/components/Button` is not a package — it is a `tsconfig.json` `paths`
    /// entry pointing at `src/components/Button`. tropism used to return
    /// [`ImportTarget::Unresolved`] for anything alias-shaped, which was honest but
    /// costly twice over: the internal edge was lost, so no rule could see it, and
    /// the unresolved count capped hygiene confidence for the whole project.
    pub path_aliases: &'a [PathAlias],
}

/// One module-resolution alias: a specifier pattern and where it points.
///
/// TypeScript allows at most one `*` per pattern and matches the longest literal
/// prefix, which is the rule implemented here. Targets are relative to the scan
/// root — the provider resolves `baseUrl` against the config file's own directory,
/// because only it knows where that file was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAlias {
    /// `@/*`, `@app/util`, `~/lib/*`.
    pub pattern: String,
    /// Candidate targets in order; the first that names a real file wins.
    pub targets: Vec<String>,
}

impl PathAlias {
    /// Substitutes `specifier` into each target, or `None` if the pattern does not
    /// match.
    pub fn expand(&self, specifier: &str) -> Option<Vec<String>> {
        match self.pattern.split_once('*') {
            None => (self.pattern == specifier).then(|| self.targets.clone()),
            Some((prefix, suffix)) => {
                let rest = specifier
                    .strip_prefix(prefix)?
                    .strip_suffix(suffix)
                    .filter(|_| specifier.len() >= prefix.len() + suffix.len())?;
                Some(
                    self.targets
                        .iter()
                        .map(|target| target.replacen('*', rest, 1))
                        .collect(),
                )
            }
        }
    }

    /// How specific this pattern is. TypeScript resolves the longest literal
    /// prefix first, so `@app/util` beats `@app/*`.
    pub fn specificity(&self) -> usize {
        self.pattern.split('*').next().unwrap_or("").len()
    }
}

impl<'a> ProjectContext<'a> {
    /// The provider's own module set, computed on first use and reused after.
    pub fn local_modules(&self, derive: impl FnOnce() -> BTreeSet<String>) -> &BTreeSet<String> {
        self.local_modules.get_or_init(derive)
    }
}

/// How a dependency reference appeared in source.
///
/// Rust forced this distinction. `anyhow::Result<T>` is a genuine use of the
/// `anyhow` crate with no `use` statement anywhere, so extracting only import
/// statements reports most of an idiomatic Rust crate's dependencies as unused.
/// But a bare path is also how local types are named — `Palette::plain()` — so a
/// path reference must never be allowed to invent a *missing* dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportForm {
    /// An explicit import statement: `use`, `import`, `require`.
    Statement,
    /// A fully-qualified path used inline. Proves usage; proves nothing about
    /// what should have been declared.
    PathReference,
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
    pub form: ImportForm,
}

impl Import {
    pub fn statement(raw: impl Into<String>, line: u32) -> Self {
        Self {
            raw: raw.into(),
            line,
            type_only: false,
            form: ImportForm::Statement,
        }
    }

    pub fn path_reference(raw: impl Into<String>, line: u32) -> Self {
        Self {
            raw: raw.into(),
            line,
            type_only: false,
            form: ImportForm::PathReference,
        }
    }

    #[must_use]
    pub fn type_only(mut self, type_only: bool) -> Self {
        self.type_only = type_only;
        self
    }
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

    /// Extensions that mark a manifest, for ecosystems that name the manifest after
    /// the project rather than by convention. .NET's `MyApp.csproj` is the case;
    /// every other language so far has a fixed filename.
    fn manifest_extensions(&self) -> &'static [&'static str] {
        &[]
    }

    /// Lockfile names, most-preferred first.
    fn lockfile_names(&self) -> &'static [&'static str];

    /// Files that can declare a workspace, beyond the manifest itself.
    ///
    /// `go.work` and `pnpm-workspace.yaml` are the cases: both state the workspace
    /// and neither is a manifest, so neither would ever be read otherwise.
    fn workspace_files(&self) -> &'static [&'static str] {
        &[]
    }

    /// The member path patterns a workspace file declares, relative to its own
    /// directory.
    ///
    /// `None` means this ecosystem does not state its workspace — which is
    /// different from `Some` with an empty member list, meaning it states one and
    /// the workspace is empty. Six of the ten return `None` always: Python, Ruby,
    /// Swift, C++, and C# have no workspace concept tropism can read without a
    /// package manager, and their projects fall back to language grouping.
    ///
    /// Files that configure module resolution, read from each project root.
    ///
    /// `tsconfig.json` is the case. It is not a manifest — it declares no
    /// dependencies and must not create a project — but it decides what a
    /// specifier means, so resolution is wrong without it.
    fn resolution_config_files(&self) -> &'static [&'static str] {
        &[]
    }

    /// Path aliases declared by one of [`Self::resolution_config_files`].
    ///
    /// `path` is the file's location relative to the scan root, so the provider can
    /// resolve config-relative settings — TypeScript's `baseUrl` — into scan-root
    /// relative targets. Pure, like every other parse hook.
    fn parse_path_aliases(&self, _path: &Utf8Path, _text: &str) -> Vec<PathAlias> {
        Vec::new()
    }

    /// Pure, like `extract_imports`: same text in, same members out.
    fn workspace_members(
        &self,
        _path: &Utf8Path,
        _text: &str,
    ) -> Option<crate::workspace::WorkspaceDecl> {
        None
    }

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

    /// The module *inside another project* that an import refers to.
    ///
    /// `resolve_import` answers "which package does this reach", which is enough for
    /// hygiene but loses the module once an edge crosses a project boundary — the
    /// repo-wide cycle graph then knows only that two packages are circular, not
    /// which modules make them so.
    ///
    /// `target` is the context of the project being reached into. Returning `None`
    /// is honest and falls back to package granularity; a wrong answer would name
    /// the wrong module in a finding, so guessing is worse than declining.
    fn resolve_cross_project(
        &self,
        _import: &Import,
        _target: &ProjectContext<'_>,
    ) -> Option<String> {
        None
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
