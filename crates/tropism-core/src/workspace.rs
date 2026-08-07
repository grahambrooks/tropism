//! What bounds a set of mutually-importable projects.
//!
//! A monorepo's projects are not all siblings of one another. `packages/web`
//! importing `@b/kit` resolves only if `@b/kit` is in the *same* workspace; a Rust
//! crate named `mylib` never satisfies a JavaScript `import 'mylib'` at all. Before
//! this module existed the sibling set was every project in the scan root,
//! regardless of language or workspace, and both of those imports were silently
//! exempted from `missing-dep` — while the rule engine, which is repo-wide by
//! design, reported the very same edge as a violation. One analysis, two answers
//! about one import.
//!
//! Membership is established in three ways, most authoritative first:
//!
//! 1. **Configured** — `[[workspaces]]` in `tropism.toml`. The user said so.
//! 2. **Declared** — the ecosystem's own statement: Cargo's `[workspace] members`,
//!    npm's `workspaces`, `pnpm-workspace.yaml`, `go.work`, Maven's `<modules>`,
//!    Gradle's `include`. Reading a file in the repository, so the hermetic
//!    constraint is untouched.
//! 3. **Language** — everything unclaimed, grouped by language. The fallback for the
//!    ecosystems that state nothing (Python, Ruby, Swift, C++, NuGet). It is
//!    strictly narrower than the old behaviour and never wider.
//!
//! Language is checked *in addition* to membership, never instead of it: even
//! inside a configured workspace, a Rust crate cannot satisfy a JavaScript import.
//! See `design/07-open-questions.md`, question 1.

use std::collections::{BTreeMap, BTreeSet};

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::model::{Language, Project};
use crate::provider::LanguageProvider;

/// What a workspace file declares. `exclude` matters for Cargo, which lets a
/// directory inside `members`' glob opt out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceDecl {
    /// Member path patterns, relative to the declaring file's directory.
    pub members: Vec<String>,
    pub exclude: Vec<String>,
}

impl WorkspaceDecl {
    pub fn members(members: impl IntoIterator<Item = String>) -> Self {
        Self {
            members: members.into_iter().collect(),
            exclude: Vec::new(),
        }
    }
}

/// A workspace boundary named in `tropism.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub root: Utf8PathBuf,
    /// Globs matching member project roots. Empty means "everything under `root`".
    pub members: Vec<String>,
}

/// How a workspace's membership was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceOrigin {
    /// `[[workspaces]]` in `tropism.toml`. Overrides inference.
    Configured,
    /// Read from the ecosystem's own workspace declaration.
    Declared,
    /// No declaration exists in this ecosystem; unclaimed projects of one language
    /// are grouped. Reported as such, because it is an inference and not a fact.
    Language,
}

impl WorkspaceOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Declared => "declared",
            Self::Language => "language",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    /// Stable display name: the root path, or `<language>` for a fallback group.
    pub id: String,
    pub root: Utf8PathBuf,
    pub origin: WorkspaceOrigin,
    /// The file that declared it, when one did. Absent for a language fallback.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_by: Option<Utf8PathBuf>,
    /// Project roots belonging to this workspace.
    pub members: Vec<Utf8PathBuf>,
    pub languages: Vec<Language>,
}

/// Every project's workspace, and the lookup that answers "are these two siblings?".
#[derive(Debug, Clone, Default)]
pub struct WorkspaceMap {
    workspaces: Vec<Workspace>,
    by_project: BTreeMap<Utf8PathBuf, usize>,
}

impl WorkspaceMap {
    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    pub fn of_project(&self, root: &Utf8Path) -> Option<&Workspace> {
        self.by_project
            .get(root)
            .and_then(|index| self.workspaces.get(*index))
    }

    /// Whether two project roots may import each other's published names without a
    /// declaration.
    pub fn same_workspace(&self, a: &Utf8Path, b: &Utf8Path) -> bool {
        match (self.by_project.get(a), self.by_project.get(b)) {
            (Some(x), Some(y)) => x == y,
            // A project outside every workspace is a sibling of nothing but itself.
            _ => a == b,
        }
    }

    /// The workspace owning a file, via the innermost project root containing it.
    pub fn of_path(&self, path: &Utf8Path, roots: &[Utf8PathBuf]) -> Option<&Workspace> {
        roots
            .iter()
            .filter(|root| root.as_str().is_empty() || path.starts_with(root))
            .max_by_key(|root| root.as_str().len())
            .and_then(|root| self.of_project(root))
    }

    pub fn is_empty(&self) -> bool {
        self.workspaces.is_empty()
    }
}

/// Reads every workspace declaration a provider can recognise in `dir`.
///
/// Returns the declaring file alongside what it declared, because
/// `tropism workspaces` reports provenance and a boundary nobody can trace back to
/// a file is one nobody can correct.
fn declarations_in(
    scan_root: &Utf8Path,
    dir: &Utf8Path,
    providers: &[&dyn LanguageProvider],
) -> Vec<(Utf8PathBuf, WorkspaceDecl)> {
    let mut found = Vec::new();
    let mut seen: BTreeSet<Utf8PathBuf> = BTreeSet::new();

    for provider in providers {
        let candidates = provider
            .manifest_names()
            .iter()
            .chain(provider.workspace_files().iter());
        for name in candidates {
            let relative = if dir.as_str().is_empty() {
                Utf8PathBuf::from(*name)
            } else {
                dir.join(name)
            };
            if !seen.insert(relative.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(scan_root.join(&relative)) else {
                continue;
            };
            if let Some(decl) = provider.workspace_members(&relative, &text) {
                found.push((relative, decl));
            }
        }
    }
    found
}

/// Expands member patterns, relative to the declaring directory, against project
/// roots that are not already claimed.
fn matching_roots(
    dir: &Utf8Path,
    decl: &WorkspaceDecl,
    candidates: &[&Project],
) -> Vec<Utf8PathBuf> {
    let compile = |patterns: &[String]| -> Vec<globset::GlobMatcher> {
        patterns
            .iter()
            .filter_map(|pattern| {
                let joined = if dir.as_str().is_empty() {
                    pattern.trim_start_matches("./").to_owned()
                } else {
                    format!("{dir}/{}", pattern.trim_start_matches("./"))
                };
                // A member pattern names a *directory*; a project root is that
                // directory, so the pattern is matched as written rather than with
                // a `/**` suffix that would also claim nested projects.
                globset::Glob::new(joined.trim_end_matches('/'))
                    .ok()
                    .map(|glob| glob.compile_matcher())
            })
            .collect()
    };

    let members = compile(&decl.members);
    let excluded = compile(&decl.exclude);

    candidates
        .iter()
        .filter(|project| {
            let path = project.root.as_std_path();
            members.iter().any(|glob| glob.is_match(path))
                && !excluded.iter().any(|glob| glob.is_match(path))
        })
        .map(|project| project.root.clone())
        .collect()
}

fn languages_of(members: &[Utf8PathBuf], projects: &[Project]) -> Vec<Language> {
    let set: BTreeSet<Language> = projects
        .iter()
        .filter(|project| members.contains(&project.root))
        .map(|project| project.language)
        .collect();
    set.into_iter().collect()
}

/// Establishes every project's workspace.
///
/// Claiming is first-come: configured boundaries win over declared ones, and an
/// outer declaration wins over an inner one, so a nested workspace cannot steal
/// members from the workspace that contains it.
pub fn resolve(
    scan_root: &Utf8Path,
    projects: &[Project],
    providers: &[&dyn LanguageProvider],
    configured: &[WorkspaceSpec],
) -> WorkspaceMap {
    let mut workspaces: Vec<Workspace> = Vec::new();
    let mut by_project: BTreeMap<Utf8PathBuf, usize> = BTreeMap::new();

    let unclaimed = |by_project: &BTreeMap<Utf8PathBuf, usize>| -> Vec<&Project> {
        projects
            .iter()
            .filter(|project| !by_project.contains_key(&project.root))
            .collect()
    };

    // 1. Configured. An empty member list means "every project under `root`",
    //    which is the shape most users will want to write.
    for spec in configured {
        let candidates = unclaimed(&by_project);
        let decl = if spec.members.is_empty() {
            WorkspaceDecl::members(["**".to_owned()])
        } else {
            WorkspaceDecl {
                members: spec.members.clone(),
                exclude: Vec::new(),
            }
        };
        let mut members = if spec.members.is_empty() {
            matching_roots(&spec.root, &decl, &candidates)
        } else {
            // Configured member globs are written relative to the scan root, not
            // to the workspace root: they sit beside `[modules]` globs in the same
            // file, and two glob dialects in one file is a trap.
            matching_roots(Utf8Path::new(""), &decl, &candidates)
        };
        // The declaring root is a member of its own workspace when it is a project.
        if candidates.iter().any(|p| p.root == spec.root) && !members.contains(&spec.root) {
            members.push(spec.root.clone());
        }
        if members.is_empty() {
            continue;
        }
        members.sort();
        let index = workspaces.len();
        for member in &members {
            by_project.insert(member.clone(), index);
        }
        workspaces.push(Workspace {
            id: if spec.root.as_str().is_empty() {
                ".".to_owned()
            } else {
                spec.root.as_str().to_owned()
            },
            root: spec.root.clone(),
            origin: WorkspaceOrigin::Configured,
            declared_by: Some(Utf8PathBuf::from(crate::rules::RULESET_FILE)),
            languages: languages_of(&members, projects),
            members,
        });
    }

    // 2. Declared. Candidate directories are the scan root plus every project root
    //    — a `go.work` sits beside no `go.mod`, so the scan root has to be looked at
    //    even when it is not itself a project.
    let mut dirs: Vec<Utf8PathBuf> = vec![Utf8PathBuf::new()];
    dirs.extend(projects.iter().map(|project| project.root.clone()));
    dirs.sort();
    dirs.dedup();
    // Outermost first, so an outer workspace claims before an inner one.
    dirs.sort_by_key(|dir| dir.as_str().len());

    for dir in dirs {
        for (file, decl) in declarations_in(scan_root, &dir, providers) {
            let candidates = unclaimed(&by_project);
            let mut members = matching_roots(&dir, &decl, &candidates);
            // The declaring directory joins its own workspace when it is a project:
            // a Cargo workspace root with a `[package]` section is a member, and an
            // npm workspace root's own `package.json` is how devDependencies hoist.
            if candidates.iter().any(|p| p.root == dir) && !members.contains(&dir) {
                members.push(dir.clone());
            }
            if members.is_empty() {
                continue;
            }
            members.sort();
            let index = workspaces.len();
            for member in &members {
                by_project.insert(member.clone(), index);
            }
            workspaces.push(Workspace {
                id: if dir.as_str().is_empty() {
                    ".".to_owned()
                } else {
                    dir.as_str().to_owned()
                },
                root: dir.clone(),
                origin: WorkspaceOrigin::Declared,
                declared_by: Some(file),
                languages: languages_of(&members, projects),
                members,
            });
        }
    }

    // 3. Language fallback for everything still unclaimed.
    let mut by_language: BTreeMap<Language, Vec<Utf8PathBuf>> = BTreeMap::new();
    for project in unclaimed(&by_project) {
        by_language
            .entry(project.language)
            .or_default()
            .push(project.root.clone());
    }
    for (language, mut members) in by_language {
        members.sort();
        let index = workspaces.len();
        for member in &members {
            by_project.insert(member.clone(), index);
        }
        workspaces.push(Workspace {
            id: format!("<{language}>"),
            root: Utf8PathBuf::new(),
            origin: WorkspaceOrigin::Language,
            declared_by: None,
            languages: vec![language],
            members,
        });
    }

    WorkspaceMap {
        workspaces,
        by_project,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(root: &str, language: Language) -> Project {
        Project {
            root: Utf8PathBuf::from(root),
            language,
            manifests: vec![],
            lockfile: None,
        }
    }

    /// The leak this module exists to close: a Rust crate must never make a
    /// JavaScript import look declared.
    #[test]
    fn a_language_fallback_never_groups_two_languages() {
        let projects = vec![
            project("rustlib", Language::Rust),
            project("jsapp", Language::JavaScript),
        ];
        let map = resolve(Utf8Path::new("."), &projects, &[], &[]);

        assert_eq!(map.workspaces().len(), 2);
        assert!(!map.same_workspace(Utf8Path::new("rustlib"), Utf8Path::new("jsapp")));
        for workspace in map.workspaces() {
            assert_eq!(workspace.origin, WorkspaceOrigin::Language);
        }
    }

    /// The other leak: two separate workspaces in one repository.
    #[test]
    fn a_configured_boundary_splits_one_language_in_two() {
        let projects = vec![
            project("serviceA/packages/web", Language::JavaScript),
            project("toolsB/packages/kit", Language::JavaScript),
        ];
        let configured = vec![
            WorkspaceSpec {
                root: Utf8PathBuf::from("serviceA"),
                members: vec![],
            },
            WorkspaceSpec {
                root: Utf8PathBuf::from("toolsB"),
                members: vec![],
            },
        ];
        let map = resolve(Utf8Path::new("."), &projects, &[], &configured);

        assert_eq!(map.workspaces().len(), 2);
        assert!(!map.same_workspace(
            Utf8Path::new("serviceA/packages/web"),
            Utf8Path::new("toolsB/packages/kit")
        ));
        assert_eq!(
            map.of_project(Utf8Path::new("serviceA/packages/web"))
                .map(|w| w.origin),
            Some(WorkspaceOrigin::Configured)
        );
    }

    #[test]
    fn members_of_one_configured_workspace_are_siblings() {
        let projects = vec![
            project("apps/web", Language::JavaScript),
            project("apps/api", Language::JavaScript),
        ];
        let configured = vec![WorkspaceSpec {
            root: Utf8PathBuf::from("apps"),
            members: vec!["apps/*".to_owned()],
        }];
        let map = resolve(Utf8Path::new("."), &projects, &[], &configured);

        assert_eq!(map.workspaces().len(), 1);
        assert!(map.same_workspace(Utf8Path::new("apps/web"), Utf8Path::new("apps/api")));
    }

    /// A configured boundary is authoritative, so it claims before inference runs.
    #[test]
    fn configured_claims_before_the_language_fallback() {
        let projects = vec![
            project("a", Language::Python),
            project("b", Language::Python),
            project("c", Language::Python),
        ];
        let configured = vec![WorkspaceSpec {
            root: Utf8PathBuf::from("a"),
            members: vec!["a".to_owned()],
        }];
        let map = resolve(Utf8Path::new("."), &projects, &[], &configured);

        assert!(!map.same_workspace(Utf8Path::new("a"), Utf8Path::new("b")));
        assert!(map.same_workspace(Utf8Path::new("b"), Utf8Path::new("c")));
    }

    #[test]
    fn an_unknown_project_is_a_sibling_of_nothing() {
        let map = WorkspaceMap::default();
        assert!(!map.same_workspace(Utf8Path::new("a"), Utf8Path::new("b")));
        assert!(map.same_workspace(Utf8Path::new("a"), Utf8Path::new("a")));
    }
}
