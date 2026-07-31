//! The end-to-end run: discover, parse, build graphs, analyze, report.
//!
//! One failure here never aborts the run. A project whose manifest will not parse
//! is recorded and skipped; the others are still analyzed. See
//! `design/01-architecture.md`, "Error handling".

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;

use crate::analysis::{self, AnalysisContext, ResolvedImport};
use crate::discovery::{self, DiscoveryError};
use crate::graph::{ModuleGraph, ModuleId};
use crate::model::Project;
use crate::provider::{ImportTarget, LanguageProvider, ProjectContext};
use crate::report::{
    CheckId, CheckStatus, Confidence, Evidence, Finding, ProjectReport, Report, Severity,
    SkippedFile,
};
use crate::rules::{DependencyEdge, EdgeLevel, ExcludeSet, PackageUse, Ruleset};

pub struct Options {
    pub respect_ignore: bool,
    /// Explicit ruleset path. `None` discovers `tropism.toml` at the scan root.
    pub rules_path: Option<Utf8PathBuf>,
    pub use_rules: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            respect_ignore: true,
            rules_path: None,
            use_rules: true,
        }
    }
}

/// The shared inputs every project's analysis needs, gathered once.
struct Pass<'a> {
    scan_root: &'a Utf8Path,
    /// Project roots, longest first, so a file in a nested project is attributed to
    /// the innermost one.
    roots: &'a [&'a Project],
    /// Names the workspace makes importable without a local declaration.
    provided: &'a [String],
    /// Package name -> the project publishing it, for cross-project edges.
    published_roots: &'a BTreeMap<String, Utf8PathBuf>,
    exclude: &'a ExcludeSet,
    options: &'a Options,
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

pub fn analyze(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
) -> Result<Report, DiscoveryError> {
    // Exclusions have to be known before anything is walked, so the ruleset is read
    // once here for its `exclude` list and again at the end for its rules.
    let exclude = if options.use_rules {
        Ruleset::discover_excludes(scan_root).unwrap_or_default()
    } else {
        ExcludeSet::default()
    };

    let projects = discovery::discover(scan_root, providers, options.respect_ignore, &exclude)?;
    let mut report = Report::new(scan_root);
    report.excluded = discovery::count_exclusions(scan_root, options.respect_ignore, &exclude)
        .into_iter()
        .map(|(pattern, matched)| crate::report::Exclusion { pattern, matched })
        .collect();

    // First pass: parse every manifest, so the second pass can tell what the
    // workspace makes available from what is genuinely undeclared.
    let parsed: Vec<(&Project, crate::model::Manifest)> = projects
        .iter()
        .filter_map(|project| {
            let provider = providers
                .iter()
                .find(|candidate| candidate.language() == project.language)?;
            let manifest_path = project.manifests.first()?;
            let text = std::fs::read_to_string(scan_root.join(manifest_path)).ok()?;
            Some((project, provider.parse_manifest(manifest_path, &text).ok()?))
        })
        .collect();

    // Longest root first, so a file inside a nested project is attributed to the
    // innermost one rather than to its parent.
    let mut roots: Vec<&Project> = projects.iter().collect();
    roots.sort_by_key(|project| std::cmp::Reverse(project.root.as_str().len()));

    // Package name -> the project that publishes it, so a dependency on a sibling
    // becomes an edge between two places in the repository rather than an external
    // package reference.
    let published_roots: BTreeMap<String, Utf8PathBuf> = parsed
        .iter()
        .filter_map(|(project, manifest)| {
            manifest
                .package_name
                .clone()
                .map(|name| (name, project.root.clone()))
        })
        .collect();

    let mut rule_input = RuleInput::default();

    for project in &projects {
        let Some(provider) = providers
            .iter()
            .find(|candidate| candidate.language() == project.language)
        else {
            continue;
        };

        // Everything the workspace makes importable here without a local
        // declaration: any sibling's published name, plus dependencies declared by
        // an ancestor project. npm hoists a monorepo root's devDependencies into a
        // shared node_modules, so a child importing `vitest` is not undeclared.
        let provided: Vec<String> = parsed
            .iter()
            .filter(|(other, _)| {
                other.root != project.root && project.root.starts_with(&other.root)
            })
            .flat_map(|(_, manifest)| manifest.deps.iter().map(|dep| dep.name.clone()))
            .chain(parsed.iter().filter_map(|(_, m)| m.package_name.clone()))
            .collect();

        let pass = Pass {
            scan_root,
            roots: &roots,
            provided: &provided,
            published_roots: &published_roots,
            exclude: &exclude,
            options,
        };

        match analyze_project(project, *provider, &pass) {
            Ok((project_report, mut skipped, mut input)) => {
                report.projects.push(project_report);
                report.skipped.append(&mut skipped);
                rule_input.edges.append(&mut input.edges);
                rule_input.uses.append(&mut input.uses);
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

    apply_rules(scan_root, &mut report, &rule_input, options);
    report.finalize();
    Ok(report)
}

/// Loads the ruleset and attaches its findings to the projects that violate it.
///
/// Rule findings land on the project containing the offending file, so the report
/// shape is unchanged; the *evaluation* is repo-wide.
fn apply_rules(scan_root: &Utf8Path, report: &mut Report, input: &RuleInput, options: &Options) {
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

    let (findings, stale) = ruleset.evaluate(&input.edges, &input.uses);

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
) -> anyhow::Result<(ProjectReport, Vec<SkippedFile>, RuleInput)> {
    let Pass {
        scan_root,
        provided,
        published_roots,
        ..
    } = *pass;
    let manifest_path = project
        .manifests
        .first()
        .ok_or_else(|| anyhow::anyhow!("project has no manifest"))?;
    let manifest_text = std::fs::read_to_string(scan_root.join(manifest_path))?;
    let manifest = provider.parse_manifest(manifest_path, &manifest_text)?;

    let resolved_tree = match &project.lockfile {
        Some(lockfile) => {
            let text = std::fs::read_to_string(scan_root.join(lockfile))?;
            provider.parse_lockfile(lockfile, &text)?
        }
        None => None,
    };

    let files = source_files(project, provider, pass);

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
    let mut parsed_files = Vec::new();
    for file in files.iter().cloned() {
        let text = match std::fs::read_to_string(scan_root.join(&file)) {
            Ok(text) => text,
            Err(error) => {
                skipped.push(SkippedFile {
                    file,
                    reason: error.to_string(),
                });
                continue;
            }
        };

        let extracted = match provider.extract_imports(&file, &text) {
            Ok(extracted) => extracted,
            Err(error) => {
                skipped.push(SkippedFile {
                    file,
                    reason: format!("{error:#}"),
                });
                continue;
            }
        };

        let default_id = module_id(&project.root, &file);
        let owner = provider.module_id_for_file(&file, &text, &default_id);
        module_graph.add_module(owner.clone());
        module_files
            .entry(owner.name.clone())
            .or_insert_with(|| file.clone());
        parsed_files.push((file, owner, extracted));
    }

    // The resolution context is built *after* the first pass, so a provider can ask
    // which modules the project defines. C# needs that to tell an internal
    // namespace from a package name.
    let known_modules: BTreeSet<String> = module_files.keys().cloned().collect();
    let ctx_for_resolution = ProjectContext {
        project,
        package_name: manifest.package_name.as_deref(),
        declared: &manifest.deps,
        sibling_packages: provided,
        known_modules: &known_modules,
        source_files: &files,
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

    let context = AnalysisContext {
        project: project.clone(),
        module_graph,
        declared: manifest.deps,
        imports,
        resolved_tree,
        // The provider's structural explanation only applies when a lockfile is
        // actually present. Telling a project with no go.sum that "go.sum is not a
        // resolved graph" names a file that is not there.
        resolved_tree_note: project
            .lockfile
            .as_ref()
            .and_then(|_| provider.resolved_tree_note())
            .map(str::to_owned),
        sibling_packages: provided.to_vec(),
    };

    let (checks, findings) = analysis::run_all(&context);
    let mut project_report = ProjectReport::new(project.clone());
    project_report.checks = checks;
    project_report.findings = findings;
    project_report.finalize();

    Ok((project_report, skipped, rule_input))
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
        .map(|path| path.strip_prefix(scan_root).unwrap_or(&path).to_owned())
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
