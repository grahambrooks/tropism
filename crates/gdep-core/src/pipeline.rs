//! The end-to-end run: discover, parse, build graphs, analyze, report.
//!
//! One failure here never aborts the run. A project whose manifest will not parse
//! is recorded and skipped; the others are still analyzed. See
//! `design/01-architecture.md`, "Error handling".

use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;

use crate::analysis::{self, AnalysisContext, ResolvedImport};
use crate::discovery::{self, DiscoveryError};
use crate::graph::{ModuleGraph, ModuleId};
use crate::model::Project;
use crate::provider::{ImportTarget, LanguageProvider, ProjectContext};
use crate::report::{CheckId, CheckStatus, ProjectReport, Report, SkippedFile};

pub struct Options {
    pub respect_ignore: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            respect_ignore: true,
        }
    }
}

pub fn analyze(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    options: &Options,
) -> Result<Report, DiscoveryError> {
    let projects = discovery::discover(scan_root, providers, options.respect_ignore)?;
    let mut report = Report::new(scan_root);

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

        match analyze_project(scan_root, project, *provider, &roots, &provided, options) {
            Ok((project_report, mut skipped)) => {
                report.projects.push(project_report);
                report.skipped.append(&mut skipped);
            }
            Err(error) => {
                // A project-level failure: record it, keep going.
                let mut project_report = ProjectReport::new(project.clone());
                for check in CheckId::ALL {
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

    report.finalize();
    Ok(report)
}

fn analyze_project(
    scan_root: &Utf8Path,
    project: &Project,
    provider: &dyn LanguageProvider,
    roots: &[&Project],
    provided: &[String],
    options: &Options,
) -> anyhow::Result<(ProjectReport, Vec<SkippedFile>)> {
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

    let files = source_files(scan_root, project, provider, roots, options);
    let ctx_for_resolution = ProjectContext {
        project,
        package_name: manifest.package_name.as_deref(),
        declared: &manifest.deps,
        sibling_packages: provided,
        source_files: &files,
    };

    let mut imports = Vec::new();
    let mut skipped = Vec::new();
    let mut module_graph = ModuleGraph::new();

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

        for import in extracted {
            let target = provider.resolve_import(&import, &file, &ctx_for_resolution);
            if let ImportTarget::Internal(target_module) = &target {
                module_graph.add_edge(owner.clone(), ModuleId::module(target_module.clone()));
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

    Ok((project_report, skipped))
}

/// Source files belonging to this project, excluding those owned by a nested one.
fn source_files(
    scan_root: &Utf8Path,
    project: &Project,
    provider: &dyn LanguageProvider,
    roots: &[&Project],
    options: &Options,
) -> Vec<Utf8PathBuf> {
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
