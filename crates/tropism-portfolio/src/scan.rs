//! One repository's files, read for what it publishes and what it links to.
//!
//! The walk honours `.gitignore`, the repository's own `tropism.toml` `exclude`
//! (so `tropism/demo/` and other deliberately broken fixtures stay out, exactly as
//! they do for `tropism analyze`), and the portfolio rules' `exclude`. Nested git
//! checkouts are skipped: each is its own repository.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use camino::{Utf8Path, Utf8PathBuf};
use tropism_core::rules::{ExcludeSet, Ruleset};

use crate::binary::{self, BinaryRef, CommandLine, FileRole, Patterns};
use crate::model::{Ecosystem, PackageRef};
use crate::{cargo, gomod, npm};

/// Files larger than this are not tool manifests or build scripts.
const MAX_TEXT: u64 = 1 << 20;

#[derive(Debug, Default)]
pub struct RepoScan {
    pub packages: Vec<PackageRef>,
    pub binaries: BTreeSet<String>,
    pub cargo_manifests: Vec<(Utf8PathBuf, cargo::Manifest)>,
    pub cargo_locks: Vec<(Utf8PathBuf, Vec<cargo::LockPackage>)>,
    pub go_mods: Vec<(Utf8PathBuf, gomod::GoMod)>,
    /// `go.sum` beside each `go.mod`: the `module version` pairs it records.
    pub go_sums: BTreeMap<Utf8PathBuf, BTreeSet<(String, String)>>,
    pub npm_packages: Vec<(Utf8PathBuf, npm::PackageJson)>,
    pub npm_locks: Vec<(Utf8PathBuf, Vec<npm::LockEntry>)>,
    pub binary_refs: Vec<BinaryRef>,
    pub command_lines: Vec<CommandLine>,
    /// `(file, problem)` for every file that could not be read or parsed.
    pub notes: Vec<(Option<Utf8PathBuf>, String)>,
    /// `(pattern, source) → files excluded`.
    pub excluded: BTreeMap<(String, Utf8PathBuf), usize>,
}

/// Exclusions from outside the repository: the portfolio rules file's, matched
/// against `<repo>/<path>`.
pub struct PortfolioExclude<'a> {
    pub patterns: &'a [(String, globset::GlobMatcher)],
    pub source: &'a Utf8Path,
}

pub fn scan_repo(
    root: &Utf8Path,
    repo_name: &str,
    patterns: &Patterns,
    portfolio: &PortfolioExclude<'_>,
) -> RepoScan {
    let mut out = RepoScan::default();
    let own_exclude = match Ruleset::discover_excludes(root) {
        Ok(set) => set,
        Err(error) => {
            out.notes.push((
                Some("tropism.toml".into()),
                format!("its exclusions were not applied: {error:#}"),
            ));
            ExcludeSet::default()
        }
    };

    let walker = ignore::WalkBuilder::new(root.as_std_path())
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .parents(false)
        .require_git(false)
        .filter_entry(|entry| {
            let name = entry.file_name();
            if name == ".git" || name == "node_modules" || name == "target" || name == ".wt" {
                return false;
            }
            // A nested checkout is a repository of its own.
            !(entry.depth() > 0
                && entry.file_type().is_some_and(|t| t.is_dir())
                && entry.path().join(".git").exists())
        })
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(path) = Utf8PathBuf::from_path_buf(entry.into_path()) else {
            continue;
        };
        let Ok(rel) = path.strip_prefix(root).map(Utf8Path::to_path_buf) else {
            continue;
        };
        let Some(role) = role_of(&rel) else { continue };

        if let Some(pattern) = own_exclude.excluded_by(&rel) {
            *out.excluded
                .entry((
                    pattern.to_owned(),
                    format!("{repo_name}/tropism.toml").into(),
                ))
                .or_default() += 1;
            continue;
        }
        let qualified = format!("{repo_name}/{rel}");
        if let Some((pattern, _)) = portfolio
            .patterns
            .iter()
            .find(|(_, glob)| glob.is_match(&qualified))
        {
            *out.excluded
                .entry((pattern.clone(), portfolio.source.to_owned()))
                .or_default() += 1;
            continue;
        }
        if path.metadata().is_ok_and(|m| m.len() > MAX_TEXT) {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                out.notes.push((Some(rel), format!("unreadable: {error}")));
                continue;
            }
        };
        read_file(root, &rel, &text, role, patterns, &mut out);
    }

    if let Ok(env) = std::fs::read_to_string(root.join(".release.env")) {
        // release-kit's list of shipped binaries is the most explicit statement a
        // repository makes about what it installs, so it replaces the inferred
        // list rather than adding to it. Inference finds every example and
        // internal tool too, and colab's `m` and `demo` were then attributed as
        // uses wherever a script ran a command of that name.
        let shipped: BTreeSet<String> = env
            .lines()
            .filter_map(|line| line.trim().strip_prefix("BINS="))
            .flat_map(|bins| bins.split(','))
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_owned)
            .collect();
        if !shipped.is_empty() {
            out.binaries = shipped;
        }
    }
    out.packages.sort();
    out.packages.dedup();
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    CargoManifest,
    CargoLock,
    GoMod,
    GoSum,
    PackageJson,
    PackageLock,
    Binary(FileRole),
}

const STRUCTURED_SKIP: &[&str] = &[
    "Cargo.toml",
    "Cargo.lock",
    "package.json",
    "package-lock.json",
    "tsconfig.json",
    "deny.toml",
    "rust-toolchain.toml",
    "rustfmt.toml",
    "clippy.toml",
    "tropism.toml",
];

fn role_of(rel: &Utf8Path) -> Option<Role> {
    let name = rel.file_name()?;
    let depth = rel.components().count();
    let role = match name {
        "Cargo.toml" => Role::CargoManifest,
        "Cargo.lock" => Role::CargoLock,
        "go.mod" => Role::GoMod,
        "go.sum" => Role::GoSum,
        "package.json" => Role::PackageJson,
        "package-lock.json" => Role::PackageLock,
        ".pre-commit-config.yaml" | ".pre-commit-config.yml" if depth == 1 => {
            Role::Binary(FileRole::PreCommit)
        }
        "Brewfile" if depth == 1 => Role::Binary(FileRole::Brewfile),
        "Makefile" | "makefile" | "GNUmakefile" | "justfile" | "Justfile" if depth <= 2 => {
            Role::Binary(FileRole::Commands)
        }
        _ if name.ends_with(".mk") && depth <= 2 => Role::Binary(FileRole::Commands),
        _ if name.starts_with("Dockerfile") && depth <= 2 => Role::Binary(FileRole::Dockerfile),
        _ if (name.ends_with(".toml") || name.ends_with(".json"))
            && depth == 1
            && !STRUCTURED_SKIP.contains(&name) =>
        {
            Role::Binary(FileRole::Structured)
        }
        _ if rel.starts_with(".github/workflows")
            && (name.ends_with(".yml") || name.ends_with(".yaml")) =>
        {
            Role::Binary(FileRole::Commands)
        }
        _ if rel.starts_with("scripts")
            && depth <= 3
            && (name.ends_with(".sh") || name.ends_with(".bash")) =>
        {
            Role::Binary(FileRole::Commands)
        }
        _ => return None,
    };
    Some(role)
}

fn read_file(
    root: &Utf8Path,
    rel: &Utf8Path,
    text: &str,
    role: Role,
    patterns: &Patterns,
    out: &mut RepoScan,
) {
    let dir = rel.parent().unwrap_or(Utf8Path::new(""));
    let note = |out: &mut RepoScan, error: String| {
        out.notes
            .push((Some(rel.to_owned()), format!("not parsed: {error}")));
    };
    match role {
        Role::CargoManifest => match cargo::parse_manifest(text) {
            Ok(mut manifest) => {
                if let Some(name) = &manifest.package {
                    out.packages.push(PackageRef {
                        ecosystem: Ecosystem::Cargo,
                        name: name.clone(),
                        manifest: rel.to_owned(),
                    });
                    let src = root.join(dir).join("src");
                    if src.join("main.rs").is_file() {
                        manifest.bins.push(name.clone());
                    }
                    manifest
                        .bins
                        .extend(bin_dir_names(src.join("bin").as_std_path()));
                }
                if ships_binaries(rel) {
                    out.binaries.extend(manifest.bins.iter().cloned());
                }
                out.cargo_manifests.push((rel.to_owned(), manifest));
            }
            Err(error) => note(out, error),
        },
        Role::CargoLock => match cargo::parse_lock(text) {
            Ok(packages) => out.cargo_locks.push((rel.to_owned(), packages)),
            Err(error) => note(out, error),
        },
        Role::GoMod => {
            let parsed = gomod::parse(text);
            if let Some(module) = &parsed.module {
                out.packages.push(PackageRef {
                    ecosystem: Ecosystem::Go,
                    name: module.clone(),
                    manifest: rel.to_owned(),
                });
                let module_dir = root.join(dir);
                if !ships_binaries(rel) {
                    // An example or test module builds nothing anyone installs.
                } else if module_dir.join("main.go").is_file() {
                    let last = module
                        .rsplit('/')
                        .find(|s| !is_major_suffix(s))
                        .unwrap_or(module);
                    out.binaries.insert(last.to_owned());
                }
                if ships_binaries(rel) {
                    out.binaries
                        .extend(go_cmd_names(module_dir.join("cmd").as_std_path()));
                }
            }
            out.go_mods.push((rel.to_owned(), parsed));
        }
        Role::GoSum => {
            let pairs = text
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    let module = fields.next()?;
                    let version = fields.next()?.trim_end_matches("/go.mod");
                    Some((module.to_owned(), version.to_owned()))
                })
                .collect();
            out.go_sums.insert(rel.to_owned(), pairs);
        }
        Role::PackageJson => match npm::parse_package(text) {
            Ok(package) => {
                if let Some(name) = &package.name {
                    out.packages.push(PackageRef {
                        ecosystem: Ecosystem::Npm,
                        name: name.clone(),
                        manifest: rel.to_owned(),
                    });
                }
                if ships_binaries(rel) {
                    out.binaries.extend(package.bins.iter().cloned());
                }
                out.npm_packages.push((rel.to_owned(), package));
            }
            Err(error) => note(out, error),
        },
        Role::PackageLock => match npm::parse_lock(text) {
            Ok(entries) => out.npm_locks.push((rel.to_owned(), entries)),
            Err(error) => note(out, error),
        },
        Role::Binary(file_role) => {
            let (refs, commands) = binary::scan(&rel.to_owned(), text, file_role, patterns);
            out.binary_refs.extend(refs);
            out.command_lines.extend(commands);
        }
    }
}

/// Whether binaries built from this manifest are ones anyone installs. Examples,
/// tests, benchmarks, fixtures and demos build programs too, with names like
/// `demo` and `server` that collide with real commands.
fn ships_binaries(manifest: &Utf8Path) -> bool {
    !manifest.components().any(|c| {
        matches!(
            c.as_str(),
            "examples"
                | "example"
                | "tests"
                | "test"
                | "benches"
                | "fixtures"
                | "demo"
                | "demos"
                | "samples"
                | "testdata"
        )
    })
}

fn is_major_suffix(segment: &str) -> bool {
    segment
        .strip_prefix('v')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `src/bin/x.rs` and `src/bin/x/main.rs` are binaries named `x`.
fn bin_dir_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.is_dir() {
                path.join("main.rs")
                    .is_file()
                    .then(|| entry.file_name().to_string_lossy().into_owned())
            } else {
                path.extension()
                    .filter(|e| *e == "rs")
                    .and_then(|_| path.file_stem())
                    .map(|stem| stem.to_string_lossy().into_owned())
            }
        })
        .collect()
}

/// Go's convention: `cmd/<name>/main.go` builds a binary called `<name>`.
fn go_cmd_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().join("main.go").is_file())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_by_name_and_place() {
        let role = |p: &str| role_of(Utf8Path::new(p));
        assert_eq!(role("Cargo.toml"), Some(Role::CargoManifest));
        assert_eq!(role("crates/x/Cargo.toml"), Some(Role::CargoManifest));
        assert_eq!(role("tools.toml"), Some(Role::Binary(FileRole::Structured)));
        assert_eq!(role("prek.toml"), Some(Role::Binary(FileRole::Structured)));
        assert_eq!(role("crates/x/tools.toml"), None);
        assert_eq!(role("deny.toml"), None);
        assert_eq!(
            role(".github/workflows/ci.yml"),
            Some(Role::Binary(FileRole::Commands))
        );
        assert_eq!(
            role("scripts/demo.sh"),
            Some(Role::Binary(FileRole::Commands))
        );
        assert_eq!(role("release.mk"), Some(Role::Binary(FileRole::Commands)));
        assert_eq!(role("src/main.rs"), None);
    }
}
