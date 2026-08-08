//! `pnpm-lock.yaml`, versions 5 through 9.
//!
//! Like `yarn.lock`, this is a genuinely resolved tree with edges, which is why it
//! is worth reading rather than reporting `Unavailable`.
//!
//! **A targeted parser, not a YAML crate.** `design/08-crates.md` recorded the
//! state of the ecosystem and it has not moved: `serde_yaml` is deprecated by its
//! author, `serde_yaml_ng` and `serde_norway` are forks with no release in over a
//! year, and `saphyr` — the one actively developed option — is still pre-0.1. The
//! subset needed here is a machine-generated, two-space-indented map of maps with
//! no anchors, aliases, flow collections or multi-line scalars, so a general parser
//! would be a large dependency bought for a small, regular grammar.
//!
//! **Where the edges live moved between versions**, which is the only real
//! complexity:
//!
//! ```text
//! v5            v6                      v9
//! packages:     packages:               packages:
//!   /react/18.3.1:  /react@18.3.1:        react@18.3.1:
//!     dependencies:   dependencies:         resolution: {...}
//!       loose-envify: 1.4.0             snapshots:
//!                                         react@18.3.1:
//!                                           dependencies:
//!                                             loose-envify: 1.4.0
//! ```
//!
//! So v9's `packages:` is metadata only and its edges are in `snapshots:`. Reading
//! `packages:` alone against a v9 file would produce a graph of isolated nodes and
//! a confident, wrong "no diamonds".

use std::collections::BTreeMap;

use tropism_core::model::ResolvedDep;

/// Parses `pnpm-lock.yaml` into a resolved dependency graph.
pub fn parse(text: &str) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
    // Package key -> its dependency keys. Both sections write into this, so a v9
    // file's `snapshots` edges land on the nodes `packages` declared.
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut section = Section::Other;
    let mut package: Option<String> = None;
    let mut in_dependencies = false;
    let mut key_indent = usize::MAX;
    let mut dependency_indent = usize::MAX;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();

        if indent == 0 {
            section = match trimmed.trim_end_matches(':') {
                "packages" => Section::Packages,
                "snapshots" => Section::Snapshots,
                _ => Section::Other,
            };
            package = None;
            in_dependencies = false;
            key_indent = usize::MAX;
            dependency_indent = usize::MAX;
            continue;
        }
        if section == Section::Other {
            continue;
        }

        // The first indented line under the section fixes the depth at which
        // package keys sit, rather than assuming two spaces.
        if indent < key_indent {
            key_indent = indent;
        }

        if indent == key_indent {
            in_dependencies = false;
            dependency_indent = usize::MAX;
            package = package_key(trimmed);
            if let Some(key) = &package {
                graph.entry(key.clone()).or_default();
            }
            continue;
        }

        let Some(current) = package.clone() else {
            continue;
        };

        if in_dependencies {
            if indent > dependency_indent {
                if let Some((name, version)) = split_dependency(trimmed) {
                    graph
                        .entry(current)
                        .or_default()
                        .push(format!("{name}@{version}"));
                }
                continue;
            }
            in_dependencies = false;
        }

        // `resolution:`, `engines:`, `dev:` and friends are all siblings of
        // `dependencies:` and are skipped by not matching here.
        if trimmed == "dependencies:" || trimmed == "optionalDependencies:" {
            in_dependencies = true;
            dependency_indent = indent;
        }
    }

    if graph.is_empty() {
        return Ok(None);
    }

    // An edge naming a package the lockfile does not contain is dropped. It happens
    // for links and for peers pnpm chose not to install, and an edge to a node that
    // is not in the graph would make the tree describe something that is not there.
    let mut resolved: Vec<ResolvedDep> = graph
        .iter()
        .filter_map(|(key, dependencies)| {
            let (name, version) = key.rsplit_once('@')?;
            let mut dependencies: Vec<String> = dependencies
                .iter()
                .filter(|target| graph.contains_key(*target))
                .cloned()
                .collect();
            dependencies.sort();
            dependencies.dedup();
            Some(ResolvedDep {
                key: key.clone(),
                name: name.to_owned(),
                version: version.to_owned(),
                dependencies,
            })
        })
        .collect();

    resolved.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(Some(resolved))
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Section {
    Packages,
    Snapshots,
    Other,
}

/// Normalises a package key to `name@version`.
///
/// Handles every spelling the format has used: a leading `/` (v5, v6), `/` as the
/// name-version separator (v5), quoting, and the `(peer@version)` suffix v6 and v9
/// append to distinguish peer resolutions. The suffix is dropped so that an edge
/// naming the plain package still finds it — tropism reports on versions, and two
/// peer variants of one version are one version.
fn package_key(line: &str) -> Option<String> {
    // A key may carry an inline value — v9 writes `pkg@1.0.0: {}` for a package
    // with no dependencies — so the value has to come off before the trailing
    // colon does, or `1.0.0: {}` becomes the version.
    let raw = line.trim();
    let raw = match raw.find(": ") {
        Some(at) => &raw[..at],
        None => raw.trim_end_matches(':'),
    };
    let raw = raw.trim().trim_matches(['"', '\'']);
    let raw = raw.strip_prefix('/').unwrap_or(raw);
    let raw = raw.split('(').next().unwrap_or(raw);
    if raw.is_empty() {
        return None;
    }

    // v5 separates with `/`: `/react/18.3.1`. Later versions use `@`.
    if let Some(at) = raw.rfind('@').filter(|at| *at > 0) {
        let (name, version) = raw.split_at(at);
        return (!name.is_empty() && version.len() > 1)
            .then(|| format!("{name}@{}", &version[1..]));
    }
    let (name, version) = raw.rsplit_once('/')?;
    (!name.is_empty() && !version.is_empty()).then(|| format!("{name}@{version}"))
}

/// A `name: version` dependency entry, with any peer suffix stripped.
fn split_dependency(line: &str) -> Option<(String, String)> {
    let (name, version) = match line.strip_prefix('\'').or_else(|| line.strip_prefix('"')) {
        Some(rest) => {
            let close = rest.find(['\'', '"'])?;
            (
                &rest[..close],
                rest[close + 1..].trim_start_matches(':').trim(),
            )
        }
        None => {
            // The *first* colon separates key from value; a value may contain more
            // of them (`link:../shared`).
            let at = line.find(':')?;
            let (name, rest) = line.split_at(at);
            (name, rest[1..].trim())
        }
    };
    let version = version.trim_matches(['"', '\'']);
    let version = version.split('(').next().unwrap_or(version).trim();
    (!name.is_empty() && !version.is_empty()).then(|| (name.trim().to_owned(), version.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(resolved: &'a [ResolvedDep], name: &str) -> &'a ResolvedDep {
        resolved
            .iter()
            .find(|dep| dep.name == name)
            .unwrap_or_else(|| panic!("{name} not in {resolved:?}"))
    }

    /// v9 keeps its edges in `snapshots:`, not `packages:`. Reading only the latter
    /// would produce isolated nodes and a confident, wrong "no diamonds".
    #[test]
    fn v9_takes_its_edges_from_snapshots() {
        let text = r#"lockfileVersion: '9.0'

importers:
  .:
    dependencies:
      react:
        specifier: ^18.0.0
        version: 18.3.1

packages:

  react@18.3.1:
    resolution: {integrity: sha512-abc}
    engines: {node: '>=0.10.0'}

  loose-envify@1.4.0:
    resolution: {integrity: sha512-def}

snapshots:

  react@18.3.1:
    dependencies:
      loose-envify: 1.4.0

  loose-envify@1.4.0: {}
"#;
        let resolved = parse(text).unwrap().expect("v9 is a resolved tree");
        assert_eq!(resolved.len(), 2);
        assert_eq!(
            find(&resolved, "react").dependencies,
            vec!["loose-envify@1.4.0"]
        );
    }

    /// v6 nests dependencies inside each package and prefixes keys with `/`.
    #[test]
    fn v6_takes_its_edges_from_packages() {
        let text = r#"lockfileVersion: '6.0'

dependencies:
  react:
    specifier: ^18.0.0
    version: 18.3.1

packages:

  /react@18.3.1:
    resolution: {integrity: sha512-abc}
    dependencies:
      loose-envify: 1.4.0
    dev: false

  /loose-envify@1.4.0:
    resolution: {integrity: sha512-def}
    dev: false
"#;
        let resolved = parse(text).unwrap().unwrap();
        assert_eq!(
            find(&resolved, "react").dependencies,
            vec!["loose-envify@1.4.0"]
        );
    }

    /// v5 separates name from version with `/` rather than `@`.
    #[test]
    fn v5_keys_split_on_a_slash() {
        let text = r#"lockfileVersion: 5.4

packages:

  /react/18.3.1:
    resolution: {integrity: sha512-abc}
    dependencies:
      loose-envify: 1.4.0

  /loose-envify/1.4.0:
    resolution: {integrity: sha512-def}
"#;
        let resolved = parse(text).unwrap().unwrap();
        assert_eq!(find(&resolved, "react").version, "18.3.1");
        assert_eq!(
            find(&resolved, "react").dependencies,
            vec!["loose-envify@1.4.0"]
        );
    }

    /// A scoped name contains an `@` of its own, so the split has to be on the last.
    #[test]
    fn a_scoped_package_keeps_its_scope() {
        assert_eq!(
            package_key("  '@babel/core@7.24.0':").as_deref(),
            Some("@babel/core@7.24.0")
        );
        assert_eq!(
            package_key("  /@babel/core@7.24.0:").as_deref(),
            Some("@babel/core@7.24.0")
        );
    }

    /// Peer suffixes distinguish resolutions of the *same* version, so dropping
    /// them keeps an edge pointing at a node that exists.
    #[test]
    fn a_peer_suffix_is_not_part_of_the_version() {
        assert_eq!(
            package_key("  react-dom@18.3.1(react@18.3.1):").as_deref(),
            Some("react-dom@18.3.1")
        );
        assert_eq!(
            split_dependency("react-dom: 18.3.1(react@18.3.1)"),
            Some(("react-dom".to_owned(), "18.3.1".to_owned()))
        );
    }

    /// Two versions of one package survive, which is what `version-conflict` reads.
    #[test]
    fn a_duplicated_package_keeps_both_versions() {
        let text = r#"lockfileVersion: '9.0'
packages:
  lodash@4.17.21:
    resolution: {integrity: sha512-a}
  lodash@3.10.1:
    resolution: {integrity: sha512-b}
"#;
        let resolved = parse(text).unwrap().unwrap();
        let versions: Vec<&str> = resolved.iter().map(|dep| dep.version.as_str()).collect();
        assert_eq!(versions, vec!["3.10.1", "4.17.21"]);
    }

    /// An edge naming something the lockfile does not contain is dropped: a tree
    /// with a dangling node describes something that is not installed.
    #[test]
    fn an_edge_to_an_absent_package_is_dropped() {
        let text = r#"lockfileVersion: '9.0'
packages:
  react@18.3.1:
    resolution: {integrity: sha512-a}
snapshots:
  react@18.3.1:
    dependencies:
      never-installed: 9.9.9
"#;
        let resolved = parse(text).unwrap().unwrap();
        assert!(find(&resolved, "react").dependencies.is_empty());
    }

    #[test]
    fn a_file_with_no_packages_is_not_an_empty_graph() {
        assert!(parse("lockfileVersion: '9.0'\n").unwrap().is_none());
        assert!(parse("").unwrap().is_none());
    }
}
