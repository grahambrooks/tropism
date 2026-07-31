//! Walk a tree once and locate project roots.
//!
//! See `design/01-architecture.md`.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;

use crate::model::{Language, Project};
use crate::provider::LanguageProvider;

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("scan root `{0}` does not exist")]
    MissingRoot(Utf8PathBuf),
    #[error("scan root `{0}` is not valid UTF-8")]
    NonUtf8Root(std::path::PathBuf),
    #[error("walking `{root}` failed: {source}")]
    Walk {
        root: Utf8PathBuf,
        source: ignore::Error,
    },
}

/// Finds every project root under `scan_root`.
///
/// Honours `.gitignore` unless `respect_ignore` is false, so vendored trees and
/// `node_modules` are excluded by default. Paths in the result are relative to
/// `scan_root`.
pub fn discover(
    scan_root: &Utf8Path,
    providers: &[&dyn LanguageProvider],
    respect_ignore: bool,
) -> Result<Vec<Project>, DiscoveryError> {
    if !scan_root.exists() {
        return Err(DiscoveryError::MissingRoot(scan_root.to_owned()));
    }

    // filename -> languages that claim it. `package.json` maps to both JS and TS.
    let mut manifest_owners: BTreeMap<&str, Vec<Language>> = BTreeMap::new();
    let mut lockfile_owners: BTreeMap<&str, Vec<Language>> = BTreeMap::new();
    for provider in providers {
        for name in provider.manifest_names() {
            manifest_owners
                .entry(name)
                .or_default()
                .push(provider.language());
        }
        for name in provider.lockfile_names() {
            lockfile_owners
                .entry(name)
                .or_default()
                .push(provider.language());
        }
    }

    // (directory, language) -> manifests and lockfile found there.
    let mut found: BTreeMap<(Utf8PathBuf, Language), (Vec<Utf8PathBuf>, Option<Utf8PathBuf>)> =
        BTreeMap::new();

    let walker = WalkBuilder::new(scan_root)
        .standard_filters(respect_ignore)
        // `ignore` defaults to honouring .gitignore only inside a git repository.
        // gdep analyzes plain directories too — an exported tarball or a worktree
        // fragment — and a `vendor/` rule means the same thing either way.
        .require_git(false)
        .build();

    for entry in walker {
        let entry = entry.map_err(|source| DiscoveryError::Walk {
            root: scan_root.to_owned(),
            source,
        })?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }

        let path = Utf8Path::from_path(entry.path())
            .ok_or_else(|| DiscoveryError::NonUtf8Root(entry.path().to_owned()))?;
        let Some(file_name) = path.file_name() else {
            continue;
        };
        let relative = relativize(scan_root, path);
        let dir = relative.parent().unwrap_or(Utf8Path::new("")).to_owned();

        if let Some(languages) = manifest_owners.get(file_name) {
            for language in languages {
                found
                    .entry((dir.clone(), *language))
                    .or_default()
                    .0
                    .push(relative.clone());
            }
        }
        if let Some(languages) = lockfile_owners.get(file_name) {
            for language in languages {
                let slot = &mut found.entry((dir.clone(), *language)).or_default().1;
                if slot.is_none() {
                    *slot = Some(relative.clone());
                }
            }
        }
    }

    // A lockfile with no manifest beside it is not a project root.
    let mut projects: Vec<Project> = found
        .into_iter()
        .filter(|(_, (manifests, _))| !manifests.is_empty())
        .map(|((root, language), (mut manifests, lockfile))| {
            manifests.sort();
            Project {
                root,
                language,
                manifests,
                lockfile,
            }
        })
        .collect();

    projects.sort_by(|a, b| (&a.root, a.language).cmp(&(&b.root, b.language)));
    Ok(projects)
}

fn relativize(scan_root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    path.strip_prefix(scan_root).unwrap_or(path).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Manifest, ResolvedDep};
    use crate::provider::{Import, ImportTarget, ProjectContext, VersionOps};

    struct StubOps;
    impl VersionOps for StubOps {
        fn compare(&self, _: &str, _: &str) -> Option<std::cmp::Ordering> {
            None
        }
        fn satisfies(&self, _: &str, _: &str) -> Option<bool> {
            None
        }
    }

    struct Stub(Language, &'static [&'static str], &'static [&'static str]);

    impl LanguageProvider for Stub {
        fn language(&self) -> Language {
            self.0
        }
        fn manifest_names(&self) -> &'static [&'static str] {
            self.1
        }
        fn lockfile_names(&self) -> &'static [&'static str] {
            self.2
        }
        fn source_extensions(&self) -> &'static [&'static str] {
            &[]
        }
        fn parse_manifest(&self, _: &Utf8Path, _: &str) -> anyhow::Result<Manifest> {
            Ok(Manifest::default())
        }
        fn parse_lockfile(
            &self,
            _: &Utf8Path,
            _: &str,
        ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
            Ok(None)
        }
        fn extract_imports(&self, _: &Utf8Path, _: &str) -> anyhow::Result<Vec<Import>> {
            Ok(vec![])
        }
        fn resolve_import(&self, _: &Import, _: &Utf8Path, _: &ProjectContext<'_>) -> ImportTarget {
            ImportTarget::Unresolved {
                reason: "stub".to_owned(),
            }
        }

        fn is_stdlib(&self, _: &str) -> bool {
            false
        }
        fn version_ops(&self) -> &dyn VersionOps {
            &StubOps
        }
    }

    fn write(dir: &Utf8Path, rel: &str, body: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn temp_dir(tag: &str) -> Utf8PathBuf {
        let base = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .unwrap()
            .join(format!("gdep-discovery-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn finds_manifest_and_pairs_its_lockfile() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let dir = temp_dir("pairs");
        write(&dir, "svc/go.mod", "module example.com/svc\n");
        write(&dir, "svc/go.sum", "");

        let projects = discover(&dir, &[&go], true).unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].root, "svc");
        assert_eq!(projects[0].manifests, vec![Utf8PathBuf::from("svc/go.mod")]);
        assert_eq!(projects[0].lockfile, Some(Utf8PathBuf::from("svc/go.sum")));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_lockfile_is_not_an_error() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let dir = temp_dir("nolock");
        write(&dir, "go.mod", "module example.com/x\n");

        let projects = discover(&dir, &[&go], true).unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(
            projects[0].lockfile, None,
            "absence is normal, not a failure"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lockfile_without_manifest_is_not_a_project() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let dir = temp_dir("orphan");
        write(&dir, "stray/go.sum", "");

        assert!(discover(&dir, &[&go], true).unwrap().is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_polyglot_repo_yields_one_project_per_root() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let rust = Stub(Language::Rust, &["Cargo.toml"], &["Cargo.lock"]);
        let dir = temp_dir("polyglot");
        write(&dir, "api/go.mod", "");
        write(&dir, "engine/Cargo.toml", "");

        let projects = discover(&dir, &[&go, &rust], true).unwrap();

        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].root, "api");
        assert_eq!(projects[1].root, "engine");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn gitignored_trees_are_excluded_outside_a_git_repository() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let dir = temp_dir("gitignore");
        write(&dir, ".gitignore", "vendor/\nnode_modules/\n");
        write(&dir, "api/go.mod", "");
        write(&dir, "vendor/dep/go.mod", "");
        write(&dir, "node_modules/pkg/go.mod", "");

        let projects = discover(&dir, &[&go], true).unwrap();

        assert_eq!(projects.len(), 1, "vendored trees must not be analyzed");
        assert_eq!(projects[0].root, "api");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_ignore_includes_vendored_trees() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let dir = temp_dir("noignore");
        write(&dir, ".gitignore", "vendor/\n");
        write(&dir, "api/go.mod", "");
        write(&dir, "vendor/dep/go.mod", "");

        let projects = discover(&dir, &[&go], false).unwrap();

        assert_eq!(projects.len(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_scan_root_is_an_error() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let result = discover(Utf8Path::new("/definitely/not/here"), &[&go], true);
        assert!(matches!(result, Err(DiscoveryError::MissingRoot(_))));
    }
}
