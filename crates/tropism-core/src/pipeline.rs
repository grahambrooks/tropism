//! The end-to-end run: discover, parse, build graphs, analyze, report.
//!
//! One failure here never aborts the run. A project whose manifest will not parse
//! is recorded and skipped; the others are still analyzed. See
//! `design/01-architecture.md`, "Error handling".

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;
use rayon::prelude::*;

use crate::analysis::{self, AnalysisContext, ResolvedImport};
use crate::discovery::{self, DiscoveryError};
use crate::graph::{ModuleGraph, ModuleId};
use crate::model::{DeclaredDep, Project};
use crate::provider::{Import, ImportTarget, LanguageProvider, ProjectContext};
use crate::report::{
    CheckId, CheckStatus, Confidence, Evidence, Finding, ProjectReport, Report, Severity,
    SkippedFile,
};
use crate::rules::{DependencyEdge, EdgeLevel, ExcludeSet, PackageUse, Ruleset};
use crate::workspace::{self, WorkspaceMap};

pub struct Options {
    pub respect_ignore: bool,
    /// Explicit ruleset path. `None` discovers `tropism.toml` at the scan root.
    pub rules_path: Option<Utf8PathBuf>,
    pub use_rules: bool,
    /// Evaluate only the rules, skipping every inferred check.
    ///
    /// What `tropism check` runs, and the division
    /// `design/10-js-evaluation.md` argues for: the sound checks gate, the
    /// advisory ones inform. It also skips lockfile parsing, which on a large
    /// `package-lock.json` is the single most expensive read in the run and
    /// cannot affect a rule.
    pub rules_only: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            respect_ignore: true,
            rules_path: None,
            use_rules: true,
            rules_only: false,
        }
    }
}

/// What a `check` run is scoped to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckScope {
    /// Every file. What CI runs, and what `tropism check` with no arguments does.
    Repository,
    /// Only violations attributable to these files, relative to the scan root.
    Files(Vec<Utf8PathBuf>),
}

/// The result of a scoped rule check.
///
/// `suppressed` is the count this type exists for. A ratchet that silently
/// conceals the backlog is how a codebase ends up with two hundred violations
/// nobody remembers agreeing to, so the number is carried even though the findings
/// themselves are not.
pub struct CheckOutcome {
    pub report: Report,
    pub scope: CheckScope,
    /// Files the scope named that tropism actually knows how to read.
    pub checked_files: usize,
    pub rules_evaluated: usize,
    /// Violations elsewhere in the repository, not attributable to this change —
    /// or `None` when this run did not look.
    ///
    /// **`None` is not zero, and the distinction is the whole point.** Counting the
    /// backlog means finding it, which means parsing every file — the cost D36
    /// exists to remove. So a scoped run declines to count rather than reporting a
    /// `0` that would read as "nothing else is wrong".
    ///
    /// Same choice, for the same reason, as [`crate::report::CheckStatus`]: a
    /// consumer must always be able to tell "clean" from "never ran". A whole-
    /// repository run — `tropism check` with no arguments, which is what CI and the
    /// pre-push hook do — still counts it exactly.
    pub suppressed: Option<usize>,
    /// Set when a file list named the ruleset itself, which widens the scope to
    /// the whole repository — editing a rule can invalidate anything.
    pub widened_by_ruleset_change: bool,
}

/// Evaluates the ruleset, optionally scoped to a set of changed files.
///
/// The scoping rule is the one `design/14-incremental-checking.md` argues for:
/// **a violation is reported when the *source* end of its edge is a changed
/// file.** A rule violation is an edge between two modules, and an edge has two
/// ends; if `api/user.ts` gains an import of `data/db`, the violation is
/// attributable to `api/user.ts`. If `data/db.ts` changed and something in `api`
/// already imported it, the edge is not new and is not this commit's fault.
///
/// That asymmetry is what makes the ratchet work: a repository with two hundred
/// existing violations passes every commit that does not add a two-hundred-and-
/// first, with no baseline file to maintain and nothing to regenerate after a
/// refactor.
pub fn check(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
    scope: &CheckScope,
) -> Result<CheckOutcome, DiscoveryError> {
    let rules_only = Options {
        respect_ignore: options.respect_ignore,
        rules_path: options.rules_path.clone(),
        use_rules: options.use_rules,
        rules_only: true,
    };

    // Editing the ruleset can invalidate the whole repository, so a change to it
    // is not something an incremental scope can honestly narrow. Open question 3
    // in design/14, decided here. Computed *before* the run rather than after,
    // because it also decides how much has to be parsed.
    let ruleset_file = options
        .rules_path
        .clone()
        .unwrap_or_else(|| Utf8PathBuf::from(crate::rules::RULESET_FILE));
    let widened = matches!(scope, CheckScope::Files(files) if files.iter().any(|file| {
        file == &ruleset_file || file.file_name() == Some(crate::rules::RULESET_FILE)
    }));

    // D36: parse only what this run can report on. A violation is attributed to the
    // file at its edge's *source* end, so an unchanged file's imports cannot produce
    // a finding here — and extraction is the whole cost of the run.
    //
    // A widened scope reports on everything, so it must parse everything. Reporting
    // from a partial parse would be the silent-clean failure the tool exists to
    // prevent, in the one place where it would be least visible.
    let parse_scope: Option<BTreeSet<Utf8PathBuf>> = match scope {
        CheckScope::Repository => None,
        CheckScope::Files(_) if widened => None,
        CheckScope::Files(files) => Some(files.iter().cloned().collect()),
    };
    let mut report = analyze_scoped(scan_root, providers, &rules_only, parse_scope.as_ref())?;

    let rules_evaluated = match &options.rules_path {
        Some(path) => std::fs::read_to_string(path)
            .ok()
            .and_then(|text| crate::rules::Ruleset::parse(path.clone(), &text).ok()),
        None => crate::rules::Ruleset::discover(scan_root).ok().flatten(),
    }
    .map_or(0, |ruleset| ruleset.rule_count());

    let (checked_files, suppressed) = match scope {
        CheckScope::Repository => (count_source_files(&report), Some(0)),
        CheckScope::Files(_) if widened => (count_source_files(&report), Some(0)),
        CheckScope::Files(files) => {
            let wanted: BTreeSet<&Utf8Path> = files.iter().map(Utf8PathBuf::as_path).collect();
            let mut suppressed = 0;
            for project in &mut report.projects {
                project.findings.retain(|finding| {
                    // The first piece of evidence is the source end of the edge;
                    // `Ruleset::module_finding` puts it there deliberately.
                    let attributed = finding
                        .evidence
                        .first()
                        .is_some_and(|evidence| wanted.contains(evidence.file.as_path()));
                    if !attributed {
                        suppressed += 1;
                    }
                    attributed
                });
            }
            // Whatever was filtered here came from the manifests and the changed
            // files alone, because nothing else was parsed. Reporting that partial
            // figure as the backlog would be worse than reporting none: it would be
            // a specific, confident, wrong number.
            let counted = parse_scope.is_none().then_some(suppressed);
            (files.len(), counted)
        }
    };

    // The per-check counts described a repository-wide run; after filtering they
    // would overstate. Restating them keeps `CheckStatus` honest.
    for project in &mut report.projects {
        let counts: BTreeMap<CheckId, usize> =
            project
                .findings
                .iter()
                .fold(BTreeMap::new(), |mut acc, finding| {
                    *acc.entry(finding.check).or_default() += 1;
                    acc
                });
        for check in [CheckId::ModuleRule, CheckId::PackageRule] {
            if let Some(CheckStatus::Ran { finding_count }) = project.checks.get_mut(&check) {
                *finding_count = counts.get(&check).copied().unwrap_or(0);
            }
        }
    }

    Ok(CheckOutcome {
        report,
        scope: scope.clone(),
        checked_files,
        rules_evaluated,
        suppressed,
        widened_by_ruleset_change: widened,
    })
}

fn count_source_files(report: &Report) -> usize {
    report.projects.iter().map(|p| p.source_file_count).sum()
}

/// What `tropism workspaces` reports.
///
/// A boundary that cannot be inspected is a boundary nobody can trust or correct.
/// Every check downstream of `WorkspaceMap` — which imports need no declaration,
/// whether an edge leaves its workspace — depends on this grouping being right, and
/// until now the only way to find out what tropism had inferred was to read the
/// source.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkspaceReport {
    pub scan_root: Utf8PathBuf,
    pub workspaces: Vec<crate::workspace::Workspace>,
    /// Dependencies whose two ends are in different workspaces.
    pub crossings: Vec<Crossing>,
}

impl WorkspaceReport {
    /// Serialized here rather than by the caller, for the same reason
    /// [`Report::to_json_pretty`] is: one contract, so the CLI and the MCP server
    /// cannot answer the same question differently.
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Crossing {
    pub from_workspace: String,
    pub to_workspace: String,
    pub from: Utf8PathBuf,
    pub to: Utf8PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub label: String,
    pub level: String,
}

/// Resolves workspace boundaries and the edges that cross them.
///
/// Runs the same passes `analyze` does, over the same inputs, so what this prints
/// is what the checks actually used — not a second implementation that could drift.
pub fn workspaces(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
) -> Result<WorkspaceReport, DiscoveryError> {
    let (exclude, configured) = if options.use_rules {
        Ruleset::discover_prepass(scan_root).unwrap_or_default()
    } else {
        (ExcludeSet::default(), Vec::new())
    };

    let projects = discovery::discover(scan_root, providers, options.respect_ignore, &exclude)?;
    let map = workspace::resolve(scan_root, &projects, providers, &configured);

    // Edges come from the full pass, because a crossing is exactly the edge shape
    // the rule engine already collects.
    let rules_only = Options {
        respect_ignore: options.respect_ignore,
        rules_path: options.rules_path.clone(),
        use_rules: options.use_rules,
        rules_only: true,
    };
    let edges = collect_edges(scan_root, providers, &rules_only)?;
    let roots: Vec<Utf8PathBuf> = projects.iter().map(|p| p.root.clone()).collect();

    let mut crossings: Vec<Crossing> = edges
        .iter()
        .filter_map(|edge| {
            let from = map.of_path(&edge.from, &roots)?;
            let to = map.of_path(&edge.to, &roots)?;
            (from.id != to.id).then(|| Crossing {
                from_workspace: from.id.clone(),
                to_workspace: to.id.clone(),
                from: edge.from.clone(),
                to: edge.to.clone(),
                line: edge.line,
                label: edge.label.clone(),
                level: edge.level.as_str().to_owned(),
            })
        })
        .collect();
    crossings.sort_by(|a, b| (&a.from, a.line, &a.label).cmp(&(&b.from, b.line, &b.label)));
    crossings.dedup_by(|a, b| (&a.from, a.line, &a.label) == (&b.from, b.line, &b.label));

    Ok(WorkspaceReport {
        scan_root: scan_root.to_owned(),
        workspaces: map.workspaces().to_vec(),
        crossings,
    })
}

/// What `tropism explain` reports for one source file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExplainReport {
    pub file: Utf8PathBuf,
    pub project: Utf8PathBuf,
    pub language: crate::model::Language,
    pub module: String,
    /// The workspace the file's project belongs to, and how that was established.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<crate::workspace::Workspace>,
    pub imports: Vec<ExplainedImport>,
}

impl ExplainReport {
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExplainedImport {
    pub raw: String,
    pub line: u32,
    /// `statement` or `path-reference`. A path reference proves use and never
    /// absence, which is why the two are distinguished everywhere.
    pub form: String,
    /// `internal`, `external`, `stdlib`, or `unresolved`.
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Why it resolved this way, in one sentence.
    pub reason: String,
}

/// Explains how every import in one file was classified.
///
/// The debugging surface for everything else in this module. A resolution outcome
/// is the product of the manifest, the workspace, the module set, and the
/// provider's own rules, and when one of those is wrong the finding it produces
/// names none of them.
pub fn explain(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
    file: &Utf8Path,
) -> anyhow::Result<ExplainReport> {
    let (exclude, configured) = if options.use_rules {
        Ruleset::discover_prepass(scan_root).unwrap_or_default()
    } else {
        (ExcludeSet::default(), Vec::new())
    };
    let projects = discovery::discover(scan_root, providers, options.respect_ignore, &exclude)?;
    let map = workspace::resolve(scan_root, &projects, providers, &configured);

    let mut roots: Vec<&Project> = projects.iter().collect();
    roots.sort_by_key(|project| std::cmp::Reverse(project.root.as_str().len()));
    let owner = owning_project(file, &roots)
        .ok_or_else(|| anyhow::anyhow!("{file} is not inside any project tropism discovered"))?;
    let project = projects
        .iter()
        .find(|project| project.root == owner)
        .expect("owning_project returns a discovered root");
    let provider = *providers
        .iter()
        .find(|candidate| candidate.language() == project.language)
        .ok_or_else(|| anyhow::anyhow!("no provider for {}", project.language))?;

    let parsed = parse_manifests(scan_root, &projects, providers);
    let manifest = parsed
        .iter()
        .find(|(candidate, _)| candidate.root == project.root)
        .map(|(_, manifest)| manifest.clone())
        .unwrap_or_default();
    let provided = provided_names(project, &parsed, &map);
    let provided_list: Vec<String> = provided.keys().cloned().collect();

    let pass = Pass {
        scan_root,
        roots: &roots,
        provided: &provided,
        provided_names: &provided_list,
        published_roots: &published_roots(&parsed),
        // Only used here to enumerate the project's files; `explain` parses the one
        // file it was asked about directly.
        parse_scope: None,
        exclude: &exclude,
        options,
    };

    // The module set has to come from the whole project: resolution asks which
    // modules exist, and answering from one file would call every internal import
    // external.
    let files = source_files(project, provider, &pass);
    let mut module_files: BTreeMap<String, Utf8PathBuf> = BTreeMap::new();
    for candidate in &files {
        let Ok(text) = std::fs::read_to_string(scan_root.join(candidate)) else {
            continue;
        };
        let default_id = module_id(&project.root, candidate);
        let owner = provider.module_id_for_file(candidate, &text, &default_id);
        module_files.entry(owner.name).or_insert(candidate.clone());
    }
    let known_modules: BTreeSet<String> = module_files.keys().cloned().collect();

    let text = std::fs::read_to_string(scan_root.join(file))
        .map_err(|error| anyhow::anyhow!("{file}: {error}"))?;
    let module = provider
        .module_id_for_file(file, &text, &module_id(&project.root, file))
        .name;

    let ctx = ProjectContext {
        project,
        package_name: manifest.package_name.as_deref(),
        declared: &manifest.deps,
        sibling_packages: &provided_list,
        known_modules: &known_modules,
        source_files: &files,
        local_modules: Default::default(),
        path_aliases: &read_path_aliases(scan_root, project, provider),
    };

    let declared: BTreeSet<&str> = manifest.deps.iter().map(|dep| dep.name.as_str()).collect();
    let imports = provider
        .extract_imports(file, &text)?
        .into_iter()
        .map(|import| {
            let target = provider.resolve_import(&import, file, &ctx);
            let (kind, name, reason) = match &target {
                ImportTarget::Internal(module) => (
                    "internal",
                    Some(module.clone()),
                    format!("resolves to module `{module}` in this project"),
                ),
                ImportTarget::External(name) => {
                    let reason = if declared.contains(name.as_str()) {
                        format!("`{name}` is declared in this project's manifest")
                    } else {
                        match provided.get(name.as_str()) {
                            Some(ProvidedBy::SelfPackage) => format!(
                                "`{name}` is this project's own published name, reached the way an \
                                 integration test reaches the library it tests"
                            ),
                            Some(ProvidedBy::WorkspaceSibling(root)) => format!(
                                "`{name}` is undeclared here, but published by `{root}` in the \
                                 same workspace — exempt from missing-dep, and disclosed"
                            ),
                            Some(ProvidedBy::HoistedFrom(root)) => format!(
                                "`{name}` is undeclared here, but declared by the ancestor \
                                 project `{root}` and reachable by upward resolution"
                            ),
                            None => format!(
                                "`{name}` is neither declared nor supplied by the workspace — \
                                 this is a missing dependency"
                            ),
                        }
                    };
                    ("external", Some(name.clone()), reason)
                }
                ImportTarget::Stdlib => (
                    "stdlib",
                    None,
                    "supplied by the language, so it needs no declaration".to_owned(),
                ),
                ImportTarget::Unresolved { reason } => ("unresolved", None, reason.clone()),
            };
            ExplainedImport {
                raw: import.raw,
                line: import.line,
                form: match import.form {
                    crate::provider::ImportForm::Statement => "statement".to_owned(),
                    crate::provider::ImportForm::PathReference => "path-reference".to_owned(),
                },
                target: kind.to_owned(),
                name,
                reason,
            }
        })
        .collect();

    Ok(ExplainReport {
        file: file.to_owned(),
        project: project.root.clone(),
        language: project.language,
        module,
        workspace: map.of_project(&project.root).cloned(),
        imports,
    })
}

/// The repo-wide dependency edges, without evaluating any rule against them.
fn collect_edges(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
) -> Result<Vec<DependencyEdge>, DiscoveryError> {
    let (exclude, configured) = if options.use_rules {
        Ruleset::discover_prepass(scan_root).unwrap_or_default()
    } else {
        (ExcludeSet::default(), Vec::new())
    };
    let projects = discovery::discover(scan_root, providers, options.respect_ignore, &exclude)?;
    let map = workspace::resolve(scan_root, &projects, providers, &configured);

    let parsed: Vec<(&Project, crate::model::Manifest)> =
        parse_manifests(scan_root, &projects, providers);
    let mut roots: Vec<&Project> = projects.iter().collect();
    roots.sort_by_key(|project| std::cmp::Reverse(project.root.as_str().len()));
    let published_roots = published_roots(&parsed);

    let mut edges = Vec::new();
    for project in &projects {
        let Some(provider) = providers
            .iter()
            .find(|candidate| candidate.language() == project.language)
        else {
            continue;
        };
        let provided = provided_names(project, &parsed, &map);
        let provided_list: Vec<String> = provided.keys().cloned().collect();
        let pass = Pass {
            scan_root,
            roots: &roots,
            provided: &provided,
            provided_names: &provided_list,
            published_roots: &published_roots,
            // `tropism workspaces` reports every crossing in the repository, so it
            // needs every edge and cannot be scoped.
            parse_scope: None,
            exclude: &exclude,
            options,
        };
        if let Ok((_, _, input, _)) = analyze_project(project, *provider, &pass) {
            edges.extend(input.edges);
        }
    }
    Ok(edges)
}

/// The lockfile an ancestor project holds for this one, if any.
///
/// D2. Cargo's `Cargo.lock` and npm's `package-lock.json` live at the workspace
/// root, and discovery pairs a lockfile only with a manifest in the *same*
/// directory — so a member reports "no lockfile found" while the root runs the
/// resolved-tree checks for the whole workspace.
///
/// This deliberately does **not** hand the ancestor's lockfile to the member. The
/// registered fix was to walk upward and adopt it, but measured against this
/// repository that would report the same 17 `Cargo.lock` findings five times over,
/// attributing a workspace-wide resolution to crates that do not own it. The
/// misleading part was always the *message*, and that is what this fixes.
fn workspace_lockfile<'a>(project: &Project, roots: &'a [&'a Project]) -> Option<&'a Utf8PathBuf> {
    roots
        .iter()
        .filter(|other| {
            other.language == project.language
                && other.root != project.root
                && project.root.starts_with(&other.root)
        })
        .filter_map(|other| other.lockfile.as_ref())
        // Innermost wins, matching how a file is attributed to a project.
        .next()
}

/// Build-tool path aliases in effect for a project.
///
/// Read per project rather than per file: a `tsconfig.json` decides what a
/// specifier *means*, so resolution is wrong without it — but it declares no
/// dependencies and must never create a project, which is why it is a
/// `resolution_config_file` rather than a manifest.
fn read_path_aliases(
    scan_root: &Utf8Path,
    project: &Project,
    provider: &dyn LanguageProvider,
) -> Vec<crate::provider::PathAlias> {
    provider
        .resolution_config_files()
        .iter()
        .flat_map(|name| {
            let relative = project.root.join(name);
            std::fs::read_to_string(scan_root.join(&relative))
                .ok()
                .map(|text| provider.parse_path_aliases(&relative, &text))
                .unwrap_or_default()
        })
        .collect()
}

/// Parses every project's manifest. A project whose manifest will not parse is
/// dropped here and reported by the pass that tries to analyze it.
fn parse_manifests<'a>(
    scan_root: &Utf8Path,
    projects: &'a [Project],
    providers: &[&dyn LanguageProvider],
) -> Vec<(&'a Project, crate::model::Manifest)> {
    projects
        .iter()
        .filter_map(|project| {
            let provider = providers
                .iter()
                .find(|candidate| candidate.language() == project.language)?;
            let manifest_path = project.manifests.first()?;
            let text = std::fs::read_to_string(scan_root.join(manifest_path)).ok()?;
            Some((project, provider.parse_manifest(manifest_path, &text).ok()?))
        })
        .collect()
}

/// Package name -> the project that publishes it, so a dependency on a sibling
/// becomes an edge between two places in the repository rather than an external
/// package reference.
fn published_roots(parsed: &[(&Project, crate::model::Manifest)]) -> BTreeMap<String, Utf8PathBuf> {
    parsed
        .iter()
        .filter_map(|(project, manifest)| {
            manifest
                .package_name
                .clone()
                .map(|name| (name, project.root.clone()))
        })
        .collect()
}

/// Everything the workspace makes importable here without a local declaration, and
/// where each name came from.
///
/// Two distinct mechanisms, kept apart because they have different bounds and the
/// report names which one applied:
///
/// * **A workspace sibling's published name.** Bounded by the workspace, because
///   `packages/web` importing `@b/kit` from a *different* workspace does not
///   resolve — npm would fail, and tropism used to say nothing.
/// * **An ancestor project's declared dependency.** Bounded by the directory tree
///   and not by the workspace, because Node's resolution walks up parent
///   directories whether or not a workspace is involved. npm hoists a monorepo
///   root's devDependencies into a shared `node_modules`, so a child importing
///   `vitest` is not undeclared.
///
/// Both are additionally bounded by language. That bound is unconditional and does
/// not defer to the workspace: a Rust crate named `mylib` must never make a
/// JavaScript `import 'mylib'` look declared, however the boundary was drawn.
fn provided_names(
    project: &Project,
    parsed: &[(&Project, crate::model::Manifest)],
    workspaces: &workspace::WorkspaceMap,
) -> BTreeMap<String, ProvidedBy> {
    let same_language = |other: &Project| other.language == project.language;
    let mut provided: BTreeMap<String, ProvidedBy> = BTreeMap::new();

    for (other, manifest) in parsed {
        if !same_language(other) {
            continue;
        }
        if other.root != project.root && project.root.starts_with(&other.root) {
            for dep in &manifest.deps {
                provided
                    .entry(dep.name.clone())
                    .or_insert_with(|| ProvidedBy::HoistedFrom(other.root.clone()));
            }
        }
    }

    for (other, manifest) in parsed {
        if !same_language(other) || !workspaces.same_workspace(&project.root, &other.root) {
            continue;
        }
        if let Some(name) = &manifest.package_name {
            let source = if other.root == project.root {
                ProvidedBy::SelfPackage
            } else {
                ProvidedBy::WorkspaceSibling(other.root.clone())
            };
            provided.insert(name.clone(), source);
        }
    }

    provided
}

/// Which mechanism supplied a name, so the exemption can be disclosed rather than
/// silently applied.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProvidedBy {
    WorkspaceSibling(Utf8PathBuf),
    HoistedFrom(Utf8PathBuf),
    /// The project's *own* published name.
    ///
    /// It has to be resolvable — Rust integration tests under `tests/` are separate
    /// crates that reach the library by its package name, and Go's `_test` packages
    /// do the same — but it is not an exemption and must not be disclosed as one.
    /// Nobody declares a dependency on themselves, so nothing was waived.
    SelfPackage,
}

/// The shared inputs every project's analysis needs, gathered once.
struct Pass<'a> {
    scan_root: &'a Utf8Path,
    /// Project roots, longest first, so a file in a nested project is attributed to
    /// the innermost one.
    roots: &'a [&'a Project],
    /// Names the workspace makes importable without a local declaration, and why.
    provided: &'a BTreeMap<String, ProvidedBy>,
    /// The same names as a flat list, which is what a provider is handed.
    provided_names: &'a [String],
    /// Package name -> the project publishing it, for cross-project edges.
    published_roots: &'a BTreeMap<String, Utf8PathBuf>,
    /// Files whose imports this run needs, or `None` for every file.
    ///
    /// This is D36. `tropism check <files>` used to walk and tree-sitter-parse the
    /// entire repository and only then narrow the *findings*, so scoping a check to
    /// one file cost exactly as much as analyzing everything — measured at 0.37 s
    /// either way on a 107-file repository, and the ratio does not improve with
    /// size. Only extraction is scoped; see `analyze_project`.
    parse_scope: Option<&'a BTreeSet<Utf8PathBuf>>,
    exclude: &'a ExcludeSet,
    options: &'a Options,
}

impl Pass<'_> {
    /// Whether this run needs the imports of `file`.
    fn parses(&self, file: &Utf8Path) -> bool {
        self.parse_scope.is_none_or(|scope| scope.contains(file))
    }
}

/// What the parallel parse pass produced for one file.
///
/// Three outcomes rather than two, because "identified but not parsed" is a real
/// state after D36: the file's module still has to reach the map, even though its
/// imports were never read.
enum FileOutcome {
    Parsed {
        file: Utf8PathBuf,
        owner: ModuleId,
        imports: Vec<Import>,
    },
    Identified {
        file: Utf8PathBuf,
        owner: ModuleId,
    },
    Skipped(SkippedFile),
}

/// Everything the rule engine needs, gathered across every project.
///
/// Rules span projects — "the CLI must not depend on the MCP server" names two
/// separate crates — so they cannot be evaluated inside the per-project pass.
#[derive(Default)]
struct RuleInput {
    edges: Vec<DependencyEdge>,
    uses: Vec<PackageUse>,
}

/// What a project looks like from outside it.
///
/// Kept so an edge reaching *into* a project can be resolved against that
/// project's own context after every project has been parsed.
struct ProjectIndex {
    project: Project,
    package_name: Option<String>,
    declared: Vec<DeclaredDep>,
    files: Vec<Utf8PathBuf>,
    modules: BTreeSet<String>,
}

pub fn analyze(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
) -> Result<Report, DiscoveryError> {
    analyze_scoped(scan_root, providers, options, None)
}

/// [`analyze`], with extraction optionally narrowed to a set of files.
///
/// `parse_scope` is D36 and is private on purpose: a report built from a partial
/// parse is only honest for the rule checks, whose findings are attributed to the
/// file at an edge's source end. [`check`] is the one caller allowed to ask for it,
/// and it always pairs it with `rules_only`.
fn analyze_scoped(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
    parse_scope: Option<&BTreeSet<Utf8PathBuf>>,
) -> Result<Report, DiscoveryError> {
    // Exclusions and workspace boundaries have to be known before anything is
    // walked, so the ruleset is read once here for both and again at the end for
    // its rules.
    let (exclude, configured_workspaces) = if options.use_rules {
        Ruleset::discover_prepass(scan_root).unwrap_or_default()
    } else {
        (ExcludeSet::default(), Vec::new())
    };

    let projects = discovery::discover(scan_root, providers, options.respect_ignore, &exclude)?;
    let mut report = Report::new(scan_root);
    report.excluded = discovery::count_exclusions(scan_root, options.respect_ignore, &exclude)
        .into_iter()
        .map(|(pattern, matched)| crate::report::Exclusion { pattern, matched })
        .collect();

    // First pass: parse every manifest, so the second pass can tell what the
    // workspace makes available from what is genuinely undeclared.
    let parsed = parse_manifests(scan_root, &projects, providers);

    // Longest root first, so a file inside a nested project is attributed to the
    // innermost one rather than to its parent.
    let mut roots: Vec<&Project> = projects.iter().collect();
    roots.sort_by_key(|project| std::cmp::Reverse(project.root.as_str().len()));

    let published_roots = published_roots(&parsed);

    // Which projects may import each other's published names without declaring
    // them. Not "all of them": see `crate::workspace` and design/07 question 1.
    let workspaces = workspace::resolve(scan_root, &projects, providers, &configured_workspaces);

    let mut rule_input = RuleInput::default();
    let mut indexes: Vec<ProjectIndex> = Vec::new();

    for project in &projects {
        let Some(provider) = providers
            .iter()
            .find(|candidate| candidate.language() == project.language)
        else {
            continue;
        };

        let provided = provided_names(project, &parsed, &workspaces);
        let provided_names: Vec<String> = provided.keys().cloned().collect();

        let pass = Pass {
            scan_root,
            roots: &roots,
            provided: &provided,
            provided_names: &provided_names,
            published_roots: &published_roots,
            parse_scope,
            exclude: &exclude,
            options,
        };

        match analyze_project(project, *provider, &pass) {
            Ok((project_report, mut skipped, mut input, index)) => {
                report.projects.push(project_report);
                report.skipped.append(&mut skipped);
                rule_input.edges.append(&mut input.edges);
                rule_input.uses.append(&mut input.uses);
                indexes.push(index);
            }
            Err(error) => {
                // A project-level failure: record it, keep going.
                let mut project_report = ProjectReport::new(project.clone());
                for check in CheckId::ANALYSIS {
                    project_report.checks.insert(
                        check,
                        CheckStatus::Failed {
                            error: format!("{error:#}"),
                        },
                    );
                }
                report.projects.push(project_report);
            }
        }
    }

    // Now that every project has been parsed, name the module each cross-project
    // edge actually reaches. Until this runs, such an edge knows only its target
    // package.
    resolve_cross_project_edges(&mut rule_input, &indexes, providers);

    // Cycles that span projects are invisible to the per-project graphs above, and
    // in a monorepo they are the ones that matter most. Not a rule, so a
    // rules-only run skips it like every other inferred check.
    if !options.rules_only {
        add_project_cycles(&mut report, &rule_input);
    }

    apply_rules(scan_root, &mut report, &rule_input, options, &workspaces);
    report.finalize();
    Ok(report)
}

/// Loads the ruleset and attaches its findings to the projects that violate it.
///
/// Rule findings land on the project containing the offending file, so the report
/// shape is unchanged; the *evaluation* is repo-wide.
/// Fills in the target module of every edge that crosses a project boundary.
///
/// `resolve_import` runs while a project is being parsed and can only see that
/// project, so an edge reaching outward knows the package it lands in but not the
/// module. Resolving it needs the *target's* context, which does not exist until
/// every project has been indexed — hence a second pass.
fn resolve_cross_project_edges(
    input: &mut RuleInput,
    indexes: &[ProjectIndex],
    providers: &[&dyn LanguageProvider],
) {
    for edge in &mut input.edges {
        if edge.to_module.is_some() || edge.from_module.is_none() {
            continue;
        }
        let Some(index) = indexes
            .iter()
            .filter(|index| {
                index.project.root.as_str().is_empty() || edge.to.starts_with(&index.project.root)
            })
            .max_by_key(|index| index.project.root.as_str().len())
        else {
            continue;
        };
        let Some(provider) = providers
            .iter()
            .find(|candidate| candidate.language() == index.project.language)
        else {
            continue;
        };

        let ctx = ProjectContext {
            project: &index.project,
            package_name: index.package_name.as_deref(),
            declared: &index.declared,
            sibling_packages: &[],
            known_modules: &index.modules,
            source_files: &index.files,
            local_modules: Default::default(),
            path_aliases: &[],
        };
        let import = Import::statement(edge.label.clone(), edge.line.unwrap_or(1));

        // `None` is a legitimate answer: the graph then falls back to the target
        // project's root, which is less precise but never wrong.
        if let Some(module) = provider.resolve_cross_project(&import, &ctx) {
            edge.to_module = Some(ModuleId::module(module).in_project(index.project.root.as_str()));
        }
    }
}

/// Detects cycles between whole projects.
///
/// `analyze_project` builds one module graph per project, so a cycle whose arms
/// live in different projects is invisible to it — the check reports `ok` while the
/// packages are mutually dependent. That is the silent-clean failure this tool is
/// otherwise organized against.
///
/// The edges are the ones the rule engine already collects, so nothing new is
/// walked or parsed: any dependency whose two ends fall in different projects is an
/// edge in a project-level graph, and Tarjan does the rest.
fn add_project_cycles(report: &mut Report, input: &RuleInput) {
    let roots: Vec<Utf8PathBuf> = report
        .projects
        .iter()
        .map(|p| p.project.root.clone())
        .collect();
    if roots.len() < 2 {
        return;
    }

    let mut graph = ModuleGraph::new();
    // One representative edge per node pair, for evidence.
    let mut witness: BTreeMap<(ModuleId, ModuleId), &DependencyEdge> = BTreeMap::new();

    for edge in &input.edges {
        let (Some(from_root), Some(to_root)) = (
            containing_root(&edge.from, &roots),
            containing_root(&edge.to, &roots),
        ) else {
            continue;
        };
        if from_root == to_root {
            continue;
        }

        // Prefer the precise module at each end; fall back to the project's root
        // module when a provider could not name one, so the edge is still
        // represented — less precise, never wrong.
        let from = edge
            .from_module
            .clone()
            .unwrap_or_else(|| ModuleId::module(".").in_project(from_root));
        let to = edge
            .to_module
            .clone()
            .unwrap_or_else(|| ModuleId::module(".").in_project(to_root));

        graph.add_edge(from.clone(), to.clone());
        witness.entry((from, to)).or_insert(edge);
    }

    for members in graph.cycles() {
        let labels: Vec<String> = members.iter().map(ModuleId::to_string).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();

        // One piece of evidence per hop, so every arm of the cycle is checkable.
        let evidence: Vec<Evidence> = members
            .iter()
            .filter_map(|member| {
                witness
                    .iter()
                    .find(|((from, to), _)| from == member && members.contains(to))
                    .map(|((_, to), edge)| {
                        Evidence::new(
                            edge.from.clone(),
                            edge.line,
                            format!("{} {} — reaches `{to}`", edge.level.as_str(), edge.label),
                        )
                    })
            })
            .collect();

        let projects: Vec<String> = members
            .iter()
            .map(|member| member.project.clone())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect();

        let finding = Finding::new(
            CheckId::Cycle,
            &Utf8PathBuf::new(),
            &refs,
            Severity::Warning,
            Confidence::High,
            format!(
                "dependency cycle across {} projects: {}",
                projects.len(),
                labels.join(" → ")
            ),
        )
        .with_evidence(evidence)
        .with_details(serde_json::json!({
            "members": labels,
            "scope": "project",
            "projects": projects,
        }));

        let owner = owning_root(&finding, report);
        if let Some(project) = report.projects.iter_mut().find(|p| p.project.root == owner) {
            // Keep the per-check count honest now that a second pass adds findings.
            if let Some(CheckStatus::Ran { finding_count }) =
                project.checks.get_mut(&CheckId::Cycle)
            {
                *finding_count += 1;
            }
            project.findings.push(finding);
        }
    }
}

/// The innermost project root containing a path.
fn containing_root(path: &Utf8Path, roots: &[Utf8PathBuf]) -> Option<String> {
    roots
        .iter()
        .filter(|root| root.as_str().is_empty() || path.starts_with(root))
        .max_by_key(|root| root.as_str().len())
        .map(|root| {
            if root.as_str().is_empty() {
                ".".to_owned()
            } else {
                root.as_str().to_owned()
            }
        })
}

fn apply_rules(
    scan_root: &Utf8Path,
    report: &mut Report,
    input: &RuleInput,
    options: &Options,
    workspaces: &WorkspaceMap,
) {
    let rule_checks = [CheckId::ModuleRule, CheckId::PackageRule];

    let loaded = if !options.use_rules {
        Err("disabled with --no-rules".to_owned())
    } else {
        match &options.rules_path {
            Some(path) => std::fs::read_to_string(path)
                .map_err(|error| format!("{path}: {error}"))
                .and_then(|text| {
                    Ruleset::parse(path.clone(), &text).map_err(|error| error.to_string())
                })
                .map(Some),
            None => Ruleset::discover(scan_root).map_err(|error| error.to_string()),
        }
    };

    let ruleset = match loaded {
        Err(error) => {
            // A broken ruleset must not look like a satisfied one.
            for project in &mut report.projects {
                for check in rule_checks {
                    project.checks.insert(
                        check,
                        CheckStatus::Failed {
                            error: error.clone(),
                        },
                    );
                }
            }
            return;
        }
        Ok(None) => {
            for project in &mut report.projects {
                for check in rule_checks {
                    project.checks.insert(
                        check,
                        CheckStatus::unavailable(format!(
                            "no {} found; see design/11-dependency-rules.md",
                            crate::rules::RULESET_FILE
                        )),
                    );
                }
            }
            return;
        }
        Ok(Some(ruleset)) => ruleset,
    };

    let project_roots: Vec<Utf8PathBuf> = report
        .projects
        .iter()
        .map(|p| p.project.root.clone())
        .collect();
    let (findings, stale) = ruleset.evaluate(&input.edges, &input.uses, workspaces, &project_roots);

    let mut counts: BTreeMap<(Utf8PathBuf, CheckId), usize> = BTreeMap::new();
    for finding in findings {
        let owner = owning_root(&finding, report);
        *counts.entry((owner.clone(), finding.check)).or_default() += 1;
        if let Some(project) = report.projects.iter_mut().find(|p| p.project.root == owner) {
            project.findings.push(finding);
        }
    }

    // A rule that had nothing to check is reported once, on the first project, so
    // a renamed module cannot silently disarm the rule that protected it.
    if let Some(first) = report.projects.first_mut() {
        for id in &stale {
            first.findings.push(
                Finding::new(
                    CheckId::ModuleRule,
                    &Utf8PathBuf::new(),
                    &["stale", id.as_str()],
                    Severity::Info,
                    Confidence::High,
                    format!("rule `{id}` matched no dependency in this repository"),
                )
                .with_evidence([Evidence::new(
                    crate::rules::RULESET_FILE,
                    None,
                    "a rule that checks nothing protects nothing",
                )])
                .with_details(serde_json::json!({ "rule_id": id, "stale": true })),
            );
            *counts
                .entry((first.project.root.clone(), CheckId::ModuleRule))
                .or_default() += 1;
        }
    }

    for project in &mut report.projects {
        for check in rule_checks {
            let count = counts
                .get(&(project.project.root.clone(), check))
                .copied()
                .unwrap_or(0);
            project.checks.insert(
                check,
                CheckStatus::Ran {
                    finding_count: count,
                },
            );
        }
    }
}

/// The project a rule finding belongs to: the innermost one containing the file it
/// cites.
fn owning_root(finding: &Finding, report: &Report) -> Utf8PathBuf {
    let file = finding
        .evidence
        .first()
        .map(|e| e.file.clone())
        .unwrap_or_default();
    report
        .projects
        .iter()
        .map(|p| p.project.root.clone())
        .filter(|root| root.as_str().is_empty() || file.starts_with(root))
        .max_by_key(|root| root.as_str().len())
        .unwrap_or_default()
}

fn analyze_project(
    project: &Project,
    provider: &dyn LanguageProvider,
    pass: &Pass<'_>,
) -> anyhow::Result<(ProjectReport, Vec<SkippedFile>, RuleInput, ProjectIndex)> {
    let Pass {
        scan_root,
        provided,
        provided_names,
        published_roots,
        ..
    } = *pass;
    let manifest_path = project
        .manifests
        .first()
        .ok_or_else(|| anyhow::anyhow!("project has no manifest"))?;
    let manifest_text = std::fs::read_to_string(scan_root.join(manifest_path))?;
    let manifest = provider.parse_manifest(manifest_path, &manifest_text)?;

    // Skipped entirely for a rules-only run. A lockfile cannot affect a rule, and
    // on a large `package-lock.json` reading and parsing it is the single most
    // expensive thing in the pass.
    let resolved_tree = match (&project.lockfile, pass.options.rules_only) {
        (Some(lockfile), false) => {
            let text = std::fs::read_to_string(scan_root.join(lockfile))?;
            provider.parse_lockfile(lockfile, &text)?
        }
        _ => None,
    };

    let files = source_files(project, provider, pass);
    let file_count = files.len();

    let path_aliases = read_path_aliases(scan_root, project, provider);

    let mut imports = Vec::new();
    let mut skipped = Vec::new();
    let mut module_graph = ModuleGraph::new();
    let mut rule_input = RuleInput::default();
    // A representative file per module, so an internal import can be turned back
    // into a path the ruleset's globs can match.
    let mut module_files: BTreeMap<String, Utf8PathBuf> = BTreeMap::new();

    // Declared dependencies are edges too: a rule broken only in a manifest is
    // still broken, and the import is one commit away.
    for dep in &manifest.deps {
        match published_roots.get(&dep.name) {
            Some(target) if target != &project.root => rule_input.edges.push(DependencyEdge {
                from: manifest_path.clone(),
                to: target.join(crate::rules::RULESET_FILE),
                line: dep.declared_at.line,
                label: dep.name.clone(),
                level: EdgeLevel::Declared,
                // A manifest entry names a package, not a module.
                from_module: None,
                to_module: None,
            }),
            Some(_) => {}
            None => rule_input.uses.push(PackageUse {
                package: dep.name.clone(),
                at: dep.declared_at.file.clone(),
                line: dep.declared_at.line,
                level: EdgeLevel::Declared,
            }),
        }
    }

    // Two passes. Module identity has to be known for *every* file before any
    // import is resolved, or an edge pointing at a file later in the walk is
    // silently dropped — which made rule findings depend on filename order.
    // D4: read, identify, and extract in parallel. Each file's work is a pure
    // function of its own bytes — `extract_imports` is documented as such, and it is
    // what makes this safe — so the only shared state is the result.
    //
    // `map` over an indexed parallel iterator preserves input order, and `files` is
    // already sorted, so the fold below sees exactly the sequence the serial version
    // did. That matters beyond tidiness: `module_files` keeps the *first* file it
    // sees for a module, so a different order would change which file a finding
    // cites. Determinism is a published property of this tool — same input, same
    // bytes — and it must not become a function of how many cores the machine has.
    let outcomes: Vec<FileOutcome> = files
        .par_iter()
        .map(|file| {
            let text = match std::fs::read_to_string(scan_root.join(file)) {
                Ok(text) => text,
                Err(error) => {
                    return FileOutcome::Skipped(SkippedFile {
                        file: file.clone(),
                        reason: error.to_string(),
                    });
                }
            };

            // Module identity for *every* file regardless of scope. It is a line
            // scan in every provider — a `namespace` or `package` declaration, or
            // pure path arithmetic — so it costs a read rather than a parse, and
            // skipping it would be wrong rather than merely faster: an internal
            // import from a changed file resolves to a module, and naming the file
            // that defines that module needs the whole map.
            let default_id = module_id(&project.root, file);
            let owner = provider.module_id_for_file(file, &text, &default_id);

            // Extraction is the expensive stage — tree-sitter over the whole file —
            // and it is the only one a scoped run can skip. A rule violation is an
            // edge attributed to the file at its *source* end, so the imports of a
            // file that did not change cannot produce a finding this run reports.
            if !pass.parses(file) {
                return FileOutcome::Identified {
                    file: file.clone(),
                    owner,
                };
            }

            match provider.extract_imports(file, &text) {
                Ok(imports) => FileOutcome::Parsed {
                    file: file.clone(),
                    owner,
                    imports,
                },
                Err(error) => FileOutcome::Skipped(SkippedFile {
                    file: file.clone(),
                    reason: format!("{error:#}"),
                }),
            }
        })
        .collect();

    let mut parsed_files = Vec::new();
    for outcome in outcomes {
        let (file, owner, imports) = match outcome {
            FileOutcome::Skipped(entry) => {
                skipped.push(entry);
                continue;
            }
            FileOutcome::Identified { file, owner } => (file, owner, None),
            FileOutcome::Parsed {
                file,
                owner,
                imports,
            } => (file, owner, Some(imports)),
        };

        module_graph.add_module(owner.clone());
        module_files
            .entry(owner.name.clone())
            .or_insert_with(|| file.clone());

        if let Some(imports) = imports {
            parsed_files.push((file, owner, imports));
        }
    }

    // The resolution context is built *after* the first pass, so a provider can ask
    // which modules the project defines. C# needs that to tell an internal
    // namespace from a package name.
    let known_modules: BTreeSet<String> = module_files.keys().cloned().collect();
    let ctx_for_resolution = ProjectContext {
        project,
        package_name: manifest.package_name.as_deref(),
        declared: &manifest.deps,
        sibling_packages: provided_names,
        known_modules: &known_modules,
        source_files: &files,
        local_modules: Default::default(),
        path_aliases: &path_aliases,
    };

    for (file, owner, extracted) in parsed_files {
        for import in extracted {
            let target = provider.resolve_import(&import, &file, &ctx_for_resolution);
            if let ImportTarget::Internal(target_module) = &target {
                module_graph.add_edge(owner.clone(), ModuleId::module(target_module.clone()));
            }
            match &target {
                ImportTarget::Internal(module) => {
                    if let Some(to) = module_files.get(module.as_str())
                        && to != &file
                    {
                        rule_input.edges.push(DependencyEdge {
                            from: file.clone(),
                            to: to.clone(),
                            line: Some(import.line),
                            label: import.raw.clone(),
                            level: EdgeLevel::Imported,
                            from_module: Some(owner.clone().in_project(project.root.as_str())),
                            to_module: Some(
                                ModuleId::module(module.clone()).in_project(project.root.as_str()),
                            ),
                        });
                    }
                }
                ImportTarget::External(name) => match published_roots.get(name.as_str()) {
                    Some(target_root) if target_root != &project.root => {
                        rule_input.edges.push(DependencyEdge {
                            from: file.clone(),
                            to: target_root.join(crate::rules::RULESET_FILE),
                            line: Some(import.line),
                            label: import.raw.clone(),
                            level: EdgeLevel::Imported,
                            from_module: Some(owner.clone().in_project(project.root.as_str())),
                            // Filled in by the cross-project pass, which needs the
                            // target project's own context.
                            to_module: None,
                        });
                    }
                    Some(_) => {}
                    None => rule_input.uses.push(PackageUse {
                        package: name.clone(),
                        at: file.clone(),
                        line: Some(import.line),
                        level: EdgeLevel::Imported,
                    }),
                },
                _ => {}
            }

            imports.push(ResolvedImport {
                file: file.clone(),
                owner: owner.clone(),
                line: import.line,
                raw: import.raw,
                target,
            });
        }
    }

    // Every import `missing-dep` will not report because the workspace supplies
    // the name. Computed here rather than in the analyzer because it is a fact
    // about resolution, and it has to be disclosed even on a `rules_only` run
    // where no analyzer executes at all.
    let exemptions = sibling_exemptions(&manifest.deps, &imports, provided);

    // Kept before `manifest.deps` is moved into the analysis context; the index
    // outlives this function so cross-project edges can be resolved later.
    let index = ProjectIndex {
        project: project.clone(),
        package_name: manifest.package_name.clone(),
        declared: manifest.deps.clone(),
        files,
        modules: known_modules,
    };

    let context = AnalysisContext {
        project: project.clone(),
        module_graph,
        declared: manifest.deps,
        imports,
        resolved_tree,
        // The provider's structural explanation only applies when a lockfile is
        // actually present. Telling a project with no go.sum that "go.sum is not a
        // resolved graph" names a file that is not there.
        //
        // When there is none, the next most useful thing is *where it actually is*.
        // That is D2: a workspace member reporting a bare "no lockfile found" while
        // the root runs the resolved-tree checks for everything is technically true
        // and actively misleading.
        resolved_tree_note: match &project.lockfile {
            Some(_) => provider.resolved_tree_note().map(str::to_owned),
            None => workspace_lockfile(project, pass.roots).map(|owner| {
                format!(
                    "no lockfile in this project; the resolved tree for this workspace is \
                     `{}`, and version-conflict and diamond-dep run on the project that \
                     owns it. A lockfile resolves the whole workspace at once, so running \
                     these checks per member would report every duplicate once per member",
                    owner
                )
            }),
        },
        sibling_packages: provided_names.to_vec(),
    };

    let mut project_report = ProjectReport::new(project.clone());
    project_report.source_file_count = file_count;
    project_report.sibling_exemptions = exemptions;
    if pass.options.rules_only {
        // Say that they did not run rather than omitting them. A consumer that
        // sees no `cycle` entry at all has to guess whether the check was clean,
        // absent, or forgotten — which is the exact ambiguity `CheckStatus`
        // exists to remove.
        for check in CheckId::ANALYSIS {
            project_report.checks.insert(
                check,
                CheckStatus::unavailable(
                    "not run: `tropism check` evaluates rules only, because they are the \
                     checks sound enough to block a commit; use `tropism analyze` for the rest",
                ),
            );
        }
    } else {
        let (checks, findings) = analysis::run_all(&context);
        project_report.checks = checks;
        project_report.findings = findings;
    }
    project_report.finalize();

    Ok((project_report, skipped, rule_input, index))
}

/// Imports that resolved to a package the manifest does not declare, which
/// `missing-dep` will pass over because the workspace supplies the name.
///
/// This is the exemption made visible. Each one is a judgement call — an undeclared
/// sibling import works today through hoisting and breaks the moment the package is
/// published or built on its own — and a judgement call nobody can see is
/// indistinguishable from a clean result.
fn sibling_exemptions(
    declared: &[DeclaredDep],
    imports: &[ResolvedImport],
    provided: &BTreeMap<String, ProvidedBy>,
) -> Vec<crate::report::SiblingExemption> {
    use crate::report::{ExemptionVia, SiblingExemption};

    let declared: BTreeSet<&str> = declared.iter().map(|dep| dep.name.as_str()).collect();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();

    for import in imports {
        if let ImportTarget::External(name) = &import.target
            && !declared.contains(name.as_str())
            // A project's own name is resolvable but waives nothing, so it is not
            // an exemption and reporting it would be noise that trains readers to
            // skip the section.
            && !matches!(
                provided.get(name.as_str()),
                None | Some(ProvidedBy::SelfPackage)
            )
        {
            *counts.entry(name.as_str()).or_default() += 1;
        }
    }

    counts
        .into_iter()
        .filter_map(|(package, imports)| {
            let (via, provided_by) = match provided.get(package)? {
                ProvidedBy::WorkspaceSibling(root) => {
                    (ExemptionVia::WorkspaceSibling, Some(root.clone()))
                }
                ProvidedBy::HoistedFrom(root) => {
                    (ExemptionVia::HoistedFromAncestor, Some(root.clone()))
                }
                ProvidedBy::SelfPackage => return None,
            };
            Some(SiblingExemption {
                package: package.to_owned(),
                via,
                provided_by,
                imports,
            })
        })
        .collect()
}

/// Source files belonging to this project, excluding those owned by a nested one.
fn source_files(
    project: &Project,
    provider: &dyn LanguageProvider,
    pass: &Pass<'_>,
) -> Vec<Utf8PathBuf> {
    let Pass {
        scan_root,
        roots,
        exclude,
        options,
        ..
    } = *pass;
    let extensions = provider.source_extensions();
    let project_dir = scan_root.join(&project.root);

    let mut files: Vec<Utf8PathBuf> = WalkBuilder::new(&project_dir)
        .standard_filters(options.respect_ignore)
        .require_git(false)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|entry| Utf8Path::from_path(entry.path()).map(Utf8Path::to_owned))
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| extensions.contains(&ext))
        })
        // Same normalization as discovery: `/` on every platform, so a provider
        // comparing an import path against this list matches on Windows too.
        .map(|path| discovery::relativize(scan_root, &path))
        .filter(|path| exclude.excluded_by(path).is_none())
        .filter(|path| owning_project(path, roots).is_some_and(|owner| owner == project.root))
        .collect();

    files.sort();
    files
}

/// The innermost project root containing `file`.
fn owning_project(file: &Utf8Path, roots: &[&Project]) -> Option<Utf8PathBuf> {
    roots
        .iter()
        .find(|project| project.root.as_str().is_empty() || file.starts_with(&project.root))
        .map(|project| project.root.clone())
}

/// Module identity for a source file: its directory relative to the project root,
/// with `"."` for the root package.
fn module_id(project_root: &Utf8Path, file: &Utf8Path) -> String {
    let relative = file.strip_prefix(project_root).unwrap_or(file);
    match relative.parent() {
        Some(parent) if !parent.as_str().is_empty() => parent.as_str().to_owned(),
        _ => ".".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Language;

    fn project(root: &str) -> Project {
        Project {
            root: Utf8PathBuf::from(root),
            language: Language::Go,
            manifests: vec![],
            lockfile: None,
        }
    }

    #[test]
    fn module_id_uses_the_directory_relative_to_the_project() {
        assert_eq!(
            module_id(Utf8Path::new("svc"), Utf8Path::new("svc/api/user.go")),
            "api"
        );
        assert_eq!(
            module_id(Utf8Path::new("svc"), Utf8Path::new("svc/api/v2/user.go")),
            "api/v2"
        );
    }

    #[test]
    fn module_id_of_a_root_level_file_is_dot() {
        assert_eq!(
            module_id(Utf8Path::new("svc"), Utf8Path::new("svc/main.go")),
            "."
        );
    }

    /// A file inside a nested project belongs to the inner one, not the outer.
    #[test]
    fn nested_projects_own_their_own_files() {
        let outer = project("");
        let inner = project("tools");
        let roots = vec![&inner, &outer]; // longest root first, as the caller sorts

        assert_eq!(
            owning_project(Utf8Path::new("tools/gen.go"), &roots),
            Some(Utf8PathBuf::from("tools"))
        );
        assert_eq!(
            owning_project(Utf8Path::new("main.go"), &roots),
            Some(Utf8PathBuf::from(""))
        );
    }
}
