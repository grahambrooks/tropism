//! Walk a tree once and locate project roots.
//!
//! See `design/01-architecture.md`.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use ignore::WalkBuilder;

use crate::model::{Language, Project};
use crate::provider::LanguageProvider;
use crate::rules::ExcludeSet;

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
    exclude: &ExcludeSet,
) -> Result<Vec<Project>, DiscoveryError> {
    if !scan_root.exists() {
        return Err(DiscoveryError::MissingRoot(scan_root.to_owned()));
    }

    // filename -> languages that claim it. `package.json` maps to both JS and TS.
    let mut manifest_owners: BTreeMap<&str, Vec<Language>> = BTreeMap::new();
    // The rank is the provider's own preference order, so a project holding more
    // than one lockfile picks the same one every run rather than whichever the
    // directory walk reached first.
    let mut lockfile_owners: BTreeMap<&str, Vec<(Language, usize)>> = BTreeMap::new();
    let mut manifest_extension_owners: BTreeMap<&str, Vec<Language>> = BTreeMap::new();
    for provider in providers {
        for name in provider.manifest_names() {
            manifest_owners
                .entry(name)
                .or_default()
                .push(provider.language());
        }
        for extension in provider.manifest_extensions() {
            manifest_extension_owners
                .entry(extension)
                .or_default()
                .push(provider.language());
        }
        for (rank, name) in provider.lockfile_names().iter().enumerate() {
            lockfile_owners
                .entry(name)
                .or_default()
                .push((provider.language(), rank));
        }
    }

    // (directory, language) -> manifests, and the best lockfile found there with
    // the rank that made it best.
    type Slot = (Vec<Utf8PathBuf>, Option<(Utf8PathBuf, usize)>);
    let mut found: BTreeMap<(Utf8PathBuf, Language), Slot> = BTreeMap::new();

    let walker = WalkBuilder::new(scan_root)
        .standard_filters(respect_ignore)
        // `ignore` defaults to honouring .gitignore only inside a git repository.
        // tropism analyzes plain directories too — an exported tarball or a worktree
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
        if exclude.excluded_by(&relative).is_some() {
            continue;
        }
        let dir = relative.parent().unwrap_or(Utf8Path::new("")).to_owned();

        let by_name = manifest_owners.get(file_name).cloned().unwrap_or_default();
        let by_extension = path
            .extension()
            .and_then(|extension| manifest_extension_owners.get(extension))
            .cloned()
            .unwrap_or_default();
        for language in by_name.into_iter().chain(by_extension) {
            found
                .entry((dir.clone(), language))
                .or_default()
                .0
                .push(relative.clone());
        }
        if let Some(languages) = lockfile_owners.get(file_name) {
            for (language, rank) in languages {
                let slot = &mut found.entry((dir.clone(), *language)).or_default().1;
                // Lower rank wins. A repository mid-migration can hold both a
                // `package-lock.json` and a `yarn.lock`, and which one tropism reads
                // must not depend on directory iteration order.
                if slot.as_ref().is_none_or(|(_, best)| rank < best) {
                    *slot = Some((relative.clone(), *rank));
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
                lockfile: lockfile.map(|(path, _)| path),
            }
        })
        .collect();

    projects.sort_by(|a, b| (&a.root, a.language).cmp(&(&b.root, b.language)));
    Ok(projects)
}

/// How many paths each exclusion pattern kept out.
///
/// A separate walk rather than a return value from [`discover`], because the count
/// is about disclosure and should not complicate the discovery signature. Walking
/// is inode-bound and cheap next to parsing. A pattern reporting zero is stale —
/// the directory was renamed and the exclusion silently stopped applying.
pub fn count_exclusions(
    scan_root: &Utf8Path,
    respect_ignore: bool,
    exclude: &ExcludeSet,
) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = exclude
        .patterns()
        .map(|pattern| (pattern.to_owned(), 0))
        .collect();
    if exclude.is_empty() {
        return counts;
    }

    let walker = WalkBuilder::new(scan_root)
        .standard_filters(respect_ignore)
        .require_git(false)
        .build();

    for entry in walker.flatten() {
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let Some(path) = Utf8Path::from_path(entry.path()) else {
            continue;
        };
        let relative = relativize(scan_root, path);
        if let Some(pattern) = exclude.excluded_by(&relative) {
            *counts.entry(pattern.to_owned()).or_default() += 1;
        }
    }
    counts
}

/// A path relative to the scan root, always with `/` separators.
///
/// Windows hands back `\`, and everything downstream is `/`-shaped: module
/// identity, the ruleset's globs, and the JSON contract. Normalizing once, here,
/// is what makes a report byte-identical across platforms — principle 5 — and it
/// is what stops a provider that joins an import path with `/` from failing to
/// match a file the walker produced with `\`. Five tests failed on Windows and
/// only on Windows before this existed.
///
/// Gated on `cfg!(windows)` rather than replacing unconditionally, because `\` is
/// a legal character in a Unix filename and rewriting it there would corrupt a
/// path rather than normalize it.
pub(crate) fn relativize(scan_root: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    let relative = path.strip_prefix(scan_root).unwrap_or(path);
    if cfg!(windows) {
        Utf8PathBuf::from(relative.as_str().replace('\\', "/"))
    } else {
        relative.to_owned()
    }
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
            .join(format!("tropism-discovery-{tag}-{}", std::process::id()));
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

        let projects = discover(&dir, &[&go], true, &ExcludeSet::default()).unwrap();

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

        let projects = discover(&dir, &[&go], true, &ExcludeSet::default()).unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(
            projects[0].lockfile, None,
            "absence is normal, not a failure"
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A repository mid-migration holds two lockfiles. Which one tropism reads
    /// decides which resolved tree every downstream check sees, so it must be the
    /// provider's stated preference and not whichever the directory walk reached
    /// first — that would make the answer depend on filesystem iteration order.
    #[test]
    fn the_preferred_lockfile_wins_when_a_project_has_several() {
        let js = Stub(
            Language::JavaScript,
            &["package.json"],
            &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"],
        );
        let dir = temp_dir("twolocks");
        write(&dir, "package.json", "{}");
        // Written least-preferred first, so a first-seen-wins implementation picks
        // the wrong one on any filesystem that preserves creation order.
        write(&dir, "yarn.lock", "");
        write(&dir, "pnpm-lock.yaml", "");
        write(&dir, "package-lock.json", "{}");

        let projects = discover(&dir, &[&js], true, &ExcludeSet::default()).unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(
            projects[0].lockfile.as_deref(),
            Some(Utf8Path::new("package-lock.json"))
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// And with the preferred one absent, the next in order is taken.
    #[test]
    fn the_next_preference_is_taken_when_the_first_is_absent() {
        let js = Stub(
            Language::JavaScript,
            &["package.json"],
            &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"],
        );
        let dir = temp_dir("onelock");
        write(&dir, "package.json", "{}");
        write(&dir, "yarn.lock", "");
        write(&dir, "pnpm-lock.yaml", "");

        let projects = discover(&dir, &[&js], true, &ExcludeSet::default()).unwrap();

        assert_eq!(
            projects[0].lockfile.as_deref(),
            Some(Utf8Path::new("pnpm-lock.yaml"))
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lockfile_without_manifest_is_not_a_project() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let dir = temp_dir("orphan");
        write(&dir, "stray/go.sum", "");

        assert!(
            discover(&dir, &[&go], true, &ExcludeSet::default())
                .unwrap()
                .is_empty()
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_polyglot_repo_yields_one_project_per_root() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let rust = Stub(Language::Rust, &["Cargo.toml"], &["Cargo.lock"]);
        let dir = temp_dir("polyglot");
        write(&dir, "api/go.mod", "");
        write(&dir, "engine/Cargo.toml", "");

        let projects = discover(&dir, &[&go, &rust], true, &ExcludeSet::default()).unwrap();

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

        let projects = discover(&dir, &[&go], true, &ExcludeSet::default()).unwrap();

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

        let projects = discover(&dir, &[&go], false, &ExcludeSet::default()).unwrap();

        assert_eq!(projects.len(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_scan_root_is_an_error() {
        let go = Stub(Language::Go, &["go.mod"], &["go.sum"]);
        let result = discover(
            Utf8Path::new("/definitely/not/here"),
            &[&go],
            true,
            &ExcludeSet::default(),
        );
        assert!(matches!(result, Err(DiscoveryError::MissingRoot(_))));
    }
}
