//! `package.json` and `package-lock.json`, read for links to the owner's repos.
//!
//! An npm dependency reaches another repository in one of two ways: a git
//! specifier (`github:o/n#v1`, `git+https://github.com/o/n.git#v1`, `o/n#v1`), or
//! a registry name that one of the owner's repositories publishes. The first is
//! certain; the second is a name match — a public package can share a name — and
//! is reported at medium confidence.

use serde_json::Value;

use crate::model::{Pin, PinKind};
use crate::text::line_of;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageJson {
    pub name: Option<String>,
    pub bins: Vec<String>,
    pub deps: Vec<Dep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    pub name: String,
    pub section: String,
    pub spec: String,
    pub line: Option<u32>,
}

const SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

pub fn parse_package(text: &str) -> Result<PackageJson, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let name = doc.get("name").and_then(Value::as_str).map(str::to_owned);
    let bins = match doc.get("bin") {
        Some(Value::String(_)) => name
            .as_deref()
            .map(|n| vec![n.rsplit('/').next().unwrap_or(n).to_owned()])
            .unwrap_or_default(),
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    };
    let mut deps = Vec::new();
    for section in SECTIONS {
        let Some(map) = doc.get(*section).and_then(Value::as_object) else {
            continue;
        };
        for (dep, spec) in map {
            let Some(spec) = spec.as_str() else { continue };
            deps.push(Dep {
                name: dep.clone(),
                section: (*section).to_owned(),
                spec: spec.to_owned(),
                line: line_of(text, &format!("\"{dep}\"")),
            });
        }
    }
    Ok(PackageJson { name, bins, deps })
}

/// A git specifier: the URL it names and the ref after `#`. `None` for a
/// registry range, a `file:`/`link:`/`workspace:` specifier, or an alias.
pub fn git_spec(spec: &str) -> Option<(String, Pin)> {
    let (url, reference) = match spec.split_once('#') {
        Some((url, reference)) => (url, Some(reference)),
        None => (spec, None),
    };
    let is_git = url.starts_with("git+")
        || url.starts_with("git://")
        || url.starts_with("github:")
        || url.starts_with("git@")
        || url.ends_with(".git")
        || (url.starts_with("https://github.com/") || url.starts_with("http://github.com/"));
    let shorthand = !url.contains(':')
        && !url.starts_with('.')
        && !url.starts_with('/')
        && !url.starts_with('@')
        && url.matches('/').count() == 1;
    let url = if is_git {
        url.to_owned()
    } else if shorthand {
        format!("github:{url}")
    } else {
        return None;
    };
    let pin = match reference {
        // `#semver:^1.2` asks npm to pick a tag.
        Some(r) if r.starts_with("semver:") => Pin::new(PinKind::Range, &r["semver:".len()..]),
        Some(r) if crate::version::stable_tag(r).is_some() => Pin::new(PinKind::Tag, r),
        Some(r) if crate::version::looks_like_commit(r) => Pin::new(PinKind::Rev, r),
        Some(r) => Pin::new(PinKind::Branch, r),
        None => Pin::new(PinKind::DefaultBranch, "HEAD"),
    };
    Some((url, pin))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    pub name: String,
    pub version: String,
    pub resolved: Option<String>,
    /// The `packages` key, e.g. `node_modules/a/node_modules/b`.
    pub location: String,
}

/// `package-lock.json` v2/v3 `packages`. v1 lockfiles (npm 6) carry a
/// `dependencies` tree instead; they are read too, one level deep.
pub fn parse_lock(text: &str) -> Result<Vec<LockEntry>, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Some(packages) = doc.get("packages").and_then(Value::as_object) {
        for (location, entry) in packages {
            let Some(name) = location
                .rsplit("node_modules/")
                .next()
                .filter(|_| !location.is_empty())
            else {
                continue;
            };
            out.push(LockEntry {
                name: entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(name)
                    .to_owned(),
                version: entry
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                resolved: entry
                    .get("resolved")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                location: location.clone(),
            });
        }
    } else if let Some(deps) = doc.get("dependencies").and_then(Value::as_object) {
        for (name, entry) in deps {
            out.push(LockEntry {
                name: name.clone(),
                version: entry
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                resolved: entry
                    .get("resolved")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                location: format!("node_modules/{name}"),
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_specifiers_in_every_spelling() {
        for (spec, kind, value) in [
            ("github:grahambrooks/lumen#v2026.11.3", "tag", "v2026.11.3"),
            ("grahambrooks/lumen#main", "branch", "main"),
            (
                "git+https://github.com/grahambrooks/lumen.git#abcdef1",
                "rev",
                "abcdef1",
            ),
            (
                "git+ssh://git@github.com/grahambrooks/lumen.git",
                "default-branch",
                "HEAD",
            ),
            ("github:grahambrooks/lumen#semver:^1.2.0", "range", "^1.2.0"),
        ] {
            let (url, pin) = git_spec(spec).unwrap_or_else(|| panic!("{spec}"));
            let id = crate::remote::parse(&url).unwrap_or_else(|| panic!("{url}"));
            assert_eq!(id.name, "lumen", "{spec}");
            assert_eq!(
                (pin.kind.as_str(), pin.value.as_str()),
                (kind, value),
                "{spec}"
            );
        }
        for spec in [
            "^1.2.0",
            "file:../x",
            "workspace:*",
            "npm:other@1",
            "@scope/pkg",
            "latest",
        ] {
            assert_eq!(git_spec(spec), None, "{spec}");
        }
    }

    #[test]
    fn package_bins_and_lock_entries() {
        let pkg = parse_package(
            r#"{"name":"@gb/tool","bin":"cli.js","dependencies":{"lumen":"github:grahambrooks/lumen#v1.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(pkg.bins, vec!["tool"]);
        assert_eq!(pkg.deps[0].name, "lumen");
        let lock = parse_lock(
            r#"{"packages":{"":{"name":"x"},"node_modules/lumen":{"version":"1.0.0","resolved":"git+ssh://git@github.com/grahambrooks/lumen.git#abc"},"node_modules/a/node_modules/lumen":{"version":"0.9.0"}}}"#,
        )
        .unwrap();
        assert_eq!(lock.len(), 2);
        assert!(lock.iter().all(|e| e.name == "lumen"));
    }
}
