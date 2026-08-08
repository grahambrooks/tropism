//! `yarn.lock`, both formats.
//!
//! Yarn is the reason D14 existed: `package-lock.json` was the only lockfile
//! JavaScript could read, so a Yarn repository reported `Unavailable` for
//! `version-conflict` and `diamond-dep` — two of six checks, in the one ecosystem
//! where a genuinely resolved tree with edges exists.
//!
//! **It is a resolved tree, and that is what makes it worth parsing.** Unlike
//! `go.sum` or `conan.lock`, every entry carries a `dependencies` block, so the
//! edges are real rather than inferred. They are stated as *ranges* rather than as
//! pointers, so resolving them means indexing the descriptors each entry answers to
//! and looking the range back up — which is the one interesting part of this file.
//!
//! Two formats, deliberately handled by one parser:
//!
//! ```text
//! v1 (classic)                     berry (v2+)
//! pkg@^1.0.0:                      "pkg@npm:^1.0.0":
//!   version "1.2.3"                  version: 1.2.3
//!   dependencies:                    dependencies:
//!     other "^2.0.0"                   other: ^2.0.0
//! ```
//!
//! The shapes differ only in punctuation, and a parser tolerant of both is smaller
//! than two parsers plus the code that decides which to run.

use std::collections::BTreeMap;

use tropism_core::model::ResolvedDep;

/// One entry, before its dependency ranges have been resolved to keys.
struct Entry {
    name: String,
    version: String,
    /// `(name, range)` pairs, exactly as written.
    dependencies: Vec<(String, String)>,
}

/// Parses `yarn.lock` into a resolved dependency graph.
///
/// `Ok(None)` when the file yields nothing usable, so the check reports
/// `Unavailable` rather than a confident empty graph.
pub fn parse(text: &str) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
    let mut entries: Vec<(Vec<String>, Entry)> = Vec::new();
    let mut current: Option<(Vec<String>, Entry)> = None;
    let mut in_dependencies = false;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();

        // Column zero starts an entry: a comma-separated list of the descriptors
        // this resolution answers to.
        if indent == 0 {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            in_dependencies = false;

            // `__metadata:` is berry's header block, not a package.
            let header = trimmed.trim_end_matches(':');
            if header.starts_with("__") {
                continue;
            }
            let descriptors: Vec<String> = header
                .split(',')
                .map(|descriptor| unquote(descriptor.trim()).to_owned())
                .filter(|descriptor| !descriptor.is_empty())
                .collect();
            if descriptors.is_empty() {
                continue;
            }
            let Some((name, _)) = split_descriptor(&descriptors[0]) else {
                continue;
            };
            current = Some((
                descriptors,
                Entry {
                    name,
                    version: String::new(),
                    dependencies: Vec::new(),
                },
            ));
            continue;
        }

        let Some((_, entry)) = current.as_mut() else {
            continue;
        };

        // A dependency line is indented one level deeper than the `dependencies:`
        // key that opened the block. Anything shallower closes it.
        if in_dependencies && indent >= 4 {
            if let Some((name, range)) = split_pair(trimmed) {
                entry.dependencies.push((name, range));
            }
            continue;
        }
        in_dependencies = false;

        if trimmed == "dependencies:" || trimmed == "optionalDependencies:" {
            in_dependencies = true;
            continue;
        }
        // `version "1.2.3"` in v1, `version: 1.2.3` in berry.
        if let Some(rest) = trimmed
            .strip_prefix("version:")
            .or_else(|| trimmed.strip_prefix("version "))
        {
            entry.version = unquote(rest.trim()).to_owned();
        }
    }
    if let Some(entry) = current.take() {
        entries.push(entry);
    }

    let entries: Vec<(Vec<String>, Entry)> = entries
        .into_iter()
        .filter(|(_, entry)| !entry.version.is_empty())
        .collect();
    if entries.is_empty() {
        return Ok(None);
    }

    // Descriptor -> key. A key is `name@version`, which identifies the *copy*:
    // Yarn installs one resolution per name and version, unlike npm, which can
    // place the same name and version at several paths.
    let mut by_descriptor: BTreeMap<&str, String> = BTreeMap::new();
    for (descriptors, entry) in &entries {
        let key = format!("{}@{}", entry.name, entry.version);
        for descriptor in descriptors {
            by_descriptor.insert(descriptor.as_str(), key.clone());
        }
    }

    let mut resolved: BTreeMap<String, ResolvedDep> = BTreeMap::new();
    for (_, entry) in &entries {
        let key = format!("{}@{}", entry.name, entry.version);
        let mut dependencies: Vec<String> = entry
            .dependencies
            .iter()
            .filter_map(|(name, range)| {
                // Berry writes the protocol into the descriptor but not into the
                // dependency range, so both spellings have to be tried. A miss is
                // dropped rather than guessed: `workspace:`, `patch:` and `portal:`
                // descriptors name things that are not registry packages.
                by_descriptor
                    .get(format!("{name}@{range}").as_str())
                    .or_else(|| by_descriptor.get(format!("{name}@npm:{range}").as_str()))
                    .cloned()
            })
            .collect();
        dependencies.sort();
        dependencies.dedup();

        resolved.entry(key.clone()).or_insert(ResolvedDep {
            key,
            name: entry.name.clone(),
            version: entry.version.clone(),
            dependencies,
        });
    }

    let mut resolved: Vec<ResolvedDep> = resolved.into_values().collect();
    resolved.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(Some(resolved))
}

/// Splits a descriptor into its package name and range.
///
/// The separator is the last `@` that is not the scope marker, so
/// `@babel/core@^7.0.0` is `@babel/core` at `^7.0.0`.
///
/// **npm aliases resolve to the package actually installed.**
/// `wrap-ansi-cjs@npm:wrap-ansi@^7.0.0` installs `wrap-ansi` under a second name,
/// so that is the name reported — otherwise `version-conflict` would treat the
/// alias as a different package and miss a duplicate that is genuinely there.
/// Leaving the raw form also put `wrap-ansi-cjs@npm:wrap-ansi` in a user-facing
/// message, which names nothing a reader can look up.
fn split_descriptor(descriptor: &str) -> Option<(String, String)> {
    let at = descriptor.rfind('@').filter(|at| *at > 0)?;
    let (name, range) = (&descriptor[..at], &descriptor[at + 1..]);
    let name = match name.split_once("@npm:") {
        Some((_alias, real)) if !real.is_empty() => real,
        _ => name,
    };
    Some((name.to_owned(), range.to_owned()))
}

/// A `name range` (v1) or `name: range` (berry) dependency line.
fn split_pair(line: &str) -> Option<(String, String)> {
    // The colon form has to be tried against the *quoted* name first, or
    // `"@scope/pkg": ^1.0.0` splits inside the scope.
    let (name, rest) = match line.strip_prefix('"') {
        Some(rest) => {
            let close = rest.find('"')?;
            (&rest[..close], rest[close + 1..].trim_start())
        }
        None => {
            let at = line.find([' ', ':'])?;
            (
                &line[..at],
                line[at..].trim_start_matches([' ', ':']).trim(),
            )
        }
    };
    let range = unquote(rest.trim_start_matches(':').trim());
    (!name.is_empty() && !range.is_empty()).then(|| (name.to_owned(), range.to_owned()))
}

fn unquote(value: &str) -> &str {
    value.trim_matches(['"', '\''])
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = r#"# THIS IS AN AUTOGENERATED FILE. DO NOT EDIT THIS FILE DIRECTLY.
# yarn lockfile v1


"@babel/code-frame@^7.0.0", "@babel/code-frame@^7.10.4":
  version "7.12.13"
  resolved "https://registry.yarnpkg.com/@babel/code-frame/-/code-frame-7.12.13.tgz"
  integrity sha512-abc==
  dependencies:
    "@babel/highlight" "^7.12.13"

"@babel/highlight@^7.12.13":
  version "7.12.13"
  resolved "https://registry.yarnpkg.com/@babel/highlight/-/highlight-7.12.13.tgz"
  integrity sha512-def==

lodash@^4.17.21:
  version "4.17.21"
  resolved "https://registry.yarnpkg.com/lodash/-/lodash-4.17.21.tgz"
  integrity sha512-ghi==
"#;

    const BERRY: &str = r#"# This file is generated by running "yarn install"
__metadata:
  version: 6
  cacheKey: 8

"@babel/code-frame@npm:^7.0.0":
  version: 7.12.13
  resolution: "@babel/code-frame@npm:7.12.13"
  dependencies:
    "@babel/highlight": ^7.12.13
  checksum: abc
  languageName: node
  linkType: hard

"@babel/highlight@npm:^7.12.13":
  version: 7.12.13
  resolution: "@babel/highlight@npm:7.12.13"
  languageName: node
  linkType: hard
"#;

    fn find<'a>(resolved: &'a [ResolvedDep], name: &str) -> &'a ResolvedDep {
        resolved
            .iter()
            .find(|dep| dep.name == name)
            .unwrap_or_else(|| panic!("{name} not in {resolved:?}"))
    }

    /// The whole point of parsing this file rather than reporting it unavailable:
    /// the edges are real, so `diamond-dep` has a graph to walk.
    #[test]
    fn v1_recovers_edges_through_the_descriptor_index() {
        let resolved = parse(V1).unwrap().expect("v1 is a resolved tree");
        assert_eq!(resolved.len(), 3);

        let frame = find(&resolved, "@babel/code-frame");
        assert_eq!(frame.version, "7.12.13");
        assert_eq!(frame.dependencies, vec!["@babel/highlight@7.12.13"]);

        // A leaf has no edges, and that is different from having unresolved ones.
        assert!(find(&resolved, "lodash").dependencies.is_empty());
    }

    /// Berry writes `npm:` into the descriptor but not into the dependency range,
    /// so the lookup has to try both spellings.
    #[test]
    fn berry_resolves_a_dependency_across_the_protocol_prefix() {
        let resolved = parse(BERRY).unwrap().expect("berry is a resolved tree");
        assert_eq!(resolved.len(), 2);
        assert_eq!(
            find(&resolved, "@babel/code-frame").dependencies,
            vec!["@babel/highlight@7.12.13"]
        );
    }

    /// An entry answering several descriptors is one installed copy, not several.
    #[test]
    fn multiple_descriptors_are_one_resolution() {
        let resolved = parse(V1).unwrap().unwrap();
        assert_eq!(
            resolved
                .iter()
                .filter(|dep| dep.name == "@babel/code-frame")
                .count(),
            1
        );
    }

    /// Two versions of one package is exactly what `version-conflict` exists to
    /// find, so it has to survive parsing.
    #[test]
    fn a_duplicated_package_keeps_both_versions() {
        let text = r#"lodash@^4.0.0:
  version "4.17.21"

lodash@^3.0.0:
  version "3.10.1"
"#;
        let resolved = parse(text).unwrap().unwrap();
        let versions: Vec<&str> = resolved
            .iter()
            .filter(|dep| dep.name == "lodash")
            .map(|dep| dep.version.as_str())
            .collect();
        assert_eq!(versions, vec!["3.10.1", "4.17.21"]);
    }

    /// A descriptor tropism cannot resolve — a workspace or patch protocol — drops
    /// the edge rather than inventing one. An edge to a package that is not in the
    /// tree would be worse than a missing edge.
    #[test]
    fn an_unresolvable_descriptor_drops_the_edge() {
        let text = r#"app@workspace:.:
  version: 0.0.0
  dependencies:
    "helper": "workspace:*"

lodash@npm:^4.0.0:
  version: 4.17.21
"#;
        let resolved = parse(text).unwrap().unwrap();
        assert!(find(&resolved, "app").dependencies.is_empty());
    }

    /// An npm alias installs one package under a second name. Reporting the alias
    /// spelling would both name something a reader cannot look up and hide a real
    /// duplicate from `version-conflict`.
    #[test]
    fn an_npm_alias_reports_the_package_actually_installed() {
        assert_eq!(
            split_descriptor("wrap-ansi-cjs@npm:wrap-ansi@^7.0.0"),
            Some(("wrap-ansi".to_owned(), "^7.0.0".to_owned()))
        );
        // A plain berry descriptor is not an alias and keeps its own name.
        assert_eq!(
            split_descriptor("wrap-ansi@npm:^8.0.0"),
            Some(("wrap-ansi".to_owned(), "npm:^8.0.0".to_owned()))
        );
        // A scope is not an alias either.
        assert_eq!(
            split_descriptor("@babel/core@^7.0.0"),
            Some(("@babel/core".to_owned(), "^7.0.0".to_owned()))
        );
    }

    /// An empty or unrecognisable file must report nothing rather than an empty
    /// graph, or the check would say "0 findings" about a tree it never had.
    #[test]
    fn an_empty_lockfile_is_not_an_empty_graph() {
        assert!(parse("# yarn lockfile v1\n").unwrap().is_none());
        assert!(parse("").unwrap().is_none());
    }
}
