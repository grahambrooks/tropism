//! `Cargo.toml` and `Cargo.lock`, read for the links between repositories.
//!
//! `tropism-lang`'s Rust provider reads the same files for a different question —
//! which crates the source imports — and keeps only names and requirements. This
//! needs what that discards: the `git` URL and the `tag`/`rev`/`branch` beside it,
//! and the source each lockfile entry was resolved from.

use crate::model::{Pin, PinKind};
use crate::text::line_of_key;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub package: Option<String>,
    /// `[[bin]]` names. Implicit binaries (`src/main.rs`, `src/bin/*.rs`) need the
    /// filesystem and are added by the scanner.
    pub bins: Vec<String>,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    /// The package name, through a `package = "…"` rename.
    pub package: String,
    pub section: String,
    pub git: Option<String>,
    pub path: Option<String>,
    pub pin: Pin,
    pub line: Option<u32>,
}

const SECTIONS: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

pub fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut manifest = Manifest {
        package: doc
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
        ..Manifest::default()
    };
    if let Some(bins) = doc.get("bin").and_then(toml::Value::as_array) {
        manifest.bins = bins
            .iter()
            .filter_map(|b| b.get("name")?.as_str().map(str::to_owned))
            .collect();
    }

    let mut tables: Vec<(String, &toml::Table)> = Vec::new();
    for section in SECTIONS {
        if let Some(table) = doc.get(*section).and_then(toml::Value::as_table) {
            tables.push(((*section).to_owned(), table));
        }
    }
    if let Some(table) = doc
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(toml::Value::as_table)
    {
        tables.push(("workspace.dependencies".to_owned(), table));
    }
    if let Some(targets) = doc.get("target").and_then(toml::Value::as_table) {
        for (cfg, target) in targets {
            for section in SECTIONS {
                if let Some(table) = target.get(*section).and_then(toml::Value::as_table) {
                    tables.push((format!("target.{cfg}.{section}"), table));
                }
            }
        }
    }

    for (section, table) in tables {
        for (key, value) in table {
            let Some(spec) = value.as_table() else {
                continue; // `name = "1.0"`: a registry requirement, never a link by URL.
            };
            let field = |name: &str| spec.get(name).and_then(toml::Value::as_str);
            let git = field("git").map(str::to_owned);
            let path = field("path").map(str::to_owned);
            if git.is_none() && path.is_none() {
                continue;
            }
            let pin = if let Some(tag) = field("tag") {
                Pin::new(PinKind::Tag, tag)
            } else if let Some(rev) = field("rev") {
                Pin::new(PinKind::Rev, rev)
            } else if let Some(branch) = field("branch") {
                Pin::new(PinKind::Branch, branch)
            } else if let Some(path) = &path {
                Pin::new(PinKind::Path, path)
            } else {
                Pin::new(PinKind::DefaultBranch, "HEAD")
            };
            manifest.deps.push(Dep {
                package: field("package").unwrap_or(key).to_owned(),
                section: section.clone(),
                git,
                path,
                pin,
                line: line_of_key(text, key),
            });
        }
    }
    Ok(manifest)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockPackage {
    pub name: String,
    pub version: String,
    pub source: Option<String>,
    pub dependencies: Vec<String>,
}

pub fn parse_lock(text: &str) -> Result<Vec<LockPackage>, String> {
    let doc: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let Some(packages) = doc.get("package").and_then(toml::Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(packages
        .iter()
        .filter_map(|p| {
            let s = |k: &str| p.get(k).and_then(toml::Value::as_str).map(str::to_owned);
            Some(LockPackage {
                name: s("name")?,
                version: s("version").unwrap_or_default(),
                source: s("source"),
                dependencies: p
                    .get("dependencies")
                    .and_then(toml::Value::as_array)
                    .map(|deps| {
                        deps.iter()
                            .filter_map(|d| d.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect())
}

/// A lockfile `source = "git+URL?tag=v1#sha"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSource {
    pub url: String,
    pub reference: Option<Pin>,
    pub commit: Option<String>,
}

pub fn git_source(source: &str) -> Option<GitSource> {
    let rest = source.strip_prefix("git+")?;
    let (rest, commit) = match rest.split_once('#') {
        Some((rest, commit)) => (rest, Some(commit.to_owned())),
        None => (rest, None),
    };
    let (url, query) = match rest.split_once('?') {
        Some((url, query)) => (url, Some(query)),
        None => (rest, None),
    };
    let reference = query.and_then(|query| {
        query.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let kind = match key {
                "tag" => PinKind::Tag,
                "rev" => PinKind::Rev,
                "branch" => PinKind::Branch,
                _ => return None,
            };
            Some(Pin::new(kind, value))
        })
    });
    Some(GitSource {
        url: url.to_owned(),
        reference,
        commit,
    })
}

/// Whether a lockfile `dependencies` entry — `name`, `name version`, or
/// `name version (source)` — names this package. Cargo adds the version and
/// source only when the name alone would be ambiguous, which is exactly the
/// two-copies case.
pub fn names(entry: &str, package: &LockPackage) -> bool {
    let mut parts = entry.splitn(3, ' ');
    if parts.next() != Some(package.name.as_str()) {
        return false;
    }
    match parts.next() {
        None => true,
        Some(version) if version != package.version => false,
        Some(_) => match parts.next() {
            None => true,
            Some(source) => {
                let source = source.trim_start_matches('(').trim_end_matches(')');
                package.source.as_deref() == Some(source)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FERRISITE: &str = r#"
[package]
name = "ferrisite"

[dependencies]
anyhow = "1"
mmd = { git = "https://github.com/grahambrooks/mmd", tag = "v2026.10.25" }
forge-dsl = { git = "https://github.com/grahambrooks/forge", tag = "v2026.10.25", default-features = false }
local = { path = "../local" }

[dev-dependencies.scene]
package = "diagram-scene"
git = "https://github.com/grahambrooks/diagram-scene"
rev = "632d4fe"

[target.'cfg(unix)'.dependencies]
puml = { git = "https://github.com/grahambrooks/puml", branch = "main" }

[workspace.dependencies]
adoc = { git = "https://github.com/grahambrooks/adoc" }
"#;

    #[test]
    fn git_and_path_dependencies_keep_their_pins() {
        let manifest = parse_manifest(FERRISITE).unwrap();
        assert_eq!(manifest.package.as_deref(), Some("ferrisite"));
        let pins: Vec<(String, String, String)> = manifest
            .deps
            .iter()
            .map(|d| {
                (
                    d.package.clone(),
                    d.pin.kind.as_str().to_owned(),
                    d.pin.value.clone(),
                )
            })
            .collect();
        let has = |p: &str, k: &str, v: &str| {
            assert!(
                pins.contains(&(p.into(), k.into(), v.into())),
                "{p} {k} {v} in {pins:?}"
            );
        };
        has("mmd", "tag", "v2026.10.25");
        has("forge-dsl", "tag", "v2026.10.25");
        has("local", "path", "../local");
        // The rename is followed: the lockfile and the library know it as this.
        has("diagram-scene", "rev", "632d4fe");
        has("puml", "branch", "main");
        has("adoc", "default-branch", "HEAD");
        assert!(!pins.iter().any(|(p, _, _)| p == "anyhow"));
        let mmd = manifest.deps.iter().find(|d| d.package == "mmd").unwrap();
        assert_eq!(mmd.line, Some(7));
    }

    #[test]
    fn lockfile_git_sources_carry_reference_and_commit() {
        let source = git_source(
            "git+https://github.com/grahambrooks/mmd?tag=v2026.10.25#a7251250b9d018b7d07bbfd64d0f0ffcd42b28d2",
        )
        .unwrap();
        assert_eq!(source.url, "https://github.com/grahambrooks/mmd");
        assert_eq!(
            source.reference,
            Some(Pin::new(PinKind::Tag, "v2026.10.25"))
        );
        assert_eq!(
            source.commit.as_deref(),
            Some("a7251250b9d018b7d07bbfd64d0f0ffcd42b28d2")
        );
        assert_eq!(
            git_source("registry+https://github.com/rust-lang/crates.io-index"),
            None
        );
    }

    #[test]
    fn a_qualified_lock_entry_names_one_copy() {
        let a = LockPackage {
            name: "diagram-scene".into(),
            version: "2026.10.25".into(),
            source: Some(
                "git+https://github.com/grahambrooks/diagram-scene?tag=v2026.10.25#aaa".into(),
            ),
            dependencies: vec![],
        };
        assert!(names("diagram-scene", &a));
        assert!(names("diagram-scene 2026.10.25", &a));
        assert!(names(
            "diagram-scene 2026.10.25 (git+https://github.com/grahambrooks/diagram-scene?tag=v2026.10.25#aaa)",
            &a
        ));
        assert!(!names("diagram-scene 2026.11.12", &a));
        assert!(!names(
            "diagram-scene 2026.10.25 (git+https://github.com/grahambrooks/diagram-scene?tag=v2026.11.12#bbb)",
            &a
        ));
        assert!(!names("diagram", &a));
    }
}
