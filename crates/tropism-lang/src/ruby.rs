//! Ruby provider.
//!
//! The `Gemfile` is the first manifest in this codebase that is *a program*. It is
//! evaluated by Bundler in a DSL context, and nothing stops it reading the
//! environment or looping over a list. So it is parsed with the Ruby grammar rather
//! than executed, taking the declarative subset — `gem` calls, and the `group`
//! blocks around them — and leaving anything dynamic out. See "Manifests that are
//! code, not data" in CLAUDE.md.
//!
//! `Gemfile.lock` is the opposite: a plain, indentation-structured record of a
//! finished resolution, with every gem's exact version and its edges. It is the
//! most straightforwardly useful lockfile of the ten.
//!
//! One consequence of Bundler's design runs through everything here: **the
//! resolution is flat**. One version of each gem, chosen for the whole
//! application. A version conflict cannot be represented in a `Gemfile.lock`
//! because Bundler refuses to write one.

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};

pub struct RubyProvider;

/// Requires satisfied by Ruby itself: core, the standard library, and the default
/// gems that ship with every interpreter.
///
/// The boundary genuinely moves — `csv` and `base64` became bundled gems in Ruby
/// 3.4, so code requiring them now needs a `gem` line. They are listed as stdlib
/// anyway: the alternative reports a missing dependency on `csv` for every
/// application that predates the change, and a false missing-dep is worse than a
/// missed one.
const STDLIB: &[&str] = &[
    "English",
    "abbrev",
    "base64",
    "benchmark",
    "bigdecimal",
    "cgi",
    "coverage",
    "csv",
    "date",
    "delegate",
    "digest",
    "drb",
    "erb",
    "etc",
    "fcntl",
    "fiddle",
    "fileutils",
    "find",
    "forwardable",
    "getoptlong",
    "io",
    "ipaddr",
    "json",
    "logger",
    "monitor",
    "mutex_m",
    "net",
    "nkf",
    "objspace",
    "observer",
    "open-uri",
    "open3",
    "openssl",
    "optparse",
    "ostruct",
    "pathname",
    "pp",
    "prettyprint",
    "prime",
    "pstore",
    "psych",
    "racc",
    "rdoc",
    "readline",
    "resolv",
    "rexml",
    "rinda",
    "ripper",
    "rss",
    "rubygems",
    "securerandom",
    "set",
    "shellwords",
    "singleton",
    "socket",
    "stringio",
    "strscan",
    "syslog",
    "tempfile",
    "time",
    "timeout",
    "tmpdir",
    "tsort",
    "un",
    "uri",
    "weakref",
    "yaml",
    "zlib",
];

/// Requires whose gem is not derivable from the path.
///
/// Ruby's convention is that `require "foo/bar"` comes from the gem `foo` or
/// `foo-bar`, and both are tried before this table. What is left is the Rails
/// family, where the file is snake_case and the gem is not.
const REQUIRE_TO_GEM: &[(&str, &str)] = &[
    ("action_cable", "actioncable"),
    ("action_mailer", "actionmailer"),
    ("action_pack", "actionpack"),
    ("action_view", "actionview"),
    ("active_job", "activejob"),
    ("active_model", "activemodel"),
    ("active_record", "activerecord"),
    ("active_storage", "activestorage"),
    ("active_support", "activesupport"),
];

struct RubyGemsOps;

impl VersionOps for RubyGemsOps {
    /// RubyGems orders a version by its dot-separated segments, comparing numbers
    /// numerically and treating a letter segment as a pre-release. Only the numeric
    /// part is needed here, and `None` on anything else keeps a wrong ordering out
    /// of a finding.
    fn compare(&self, a: &str, b: &str) -> Option<std::cmp::Ordering> {
        let parse = |version: &str| -> Option<Vec<u64>> {
            version
                .trim()
                .split('.')
                .map(|part| part.parse().ok())
                .collect::<Option<Vec<u64>>>()
                .filter(|parts| !parts.is_empty())
        };
        Some(parse(a)?.cmp(&parse(b)?))
    }

    /// Not implemented. `~>` is RubyGems' pessimistic operator and no check needs
    /// it yet; approximating it would be worse than declining.
    fn satisfies(&self, _version: &str, _requirement: &str) -> Option<bool> {
        None
    }
}

impl LanguageProvider for RubyProvider {
    fn language(&self) -> Language {
        Language::Ruby
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["Gemfile"]
    }

    fn lockfile_names(&self) -> &'static [&'static str] {
        &["Gemfile.lock"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["rb"]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        parse_gemfile(path, text)
    }

    fn parse_lockfile(
        &self,
        path: &Utf8Path,
        text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        parse_gemfile_lock(path, text)
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_ruby_requires(path, text)
    }

    /// A Ruby module is its file path without the extension, as JavaScript's is.
    ///
    /// Ruby has no compilation unit larger than the file and no declared package —
    /// `require` names a path, and two files in one directory are independent. The
    /// directory granularity most languages use would hide every cycle inside a
    /// directory, which for a Rails app is most of them.
    fn module_id_for_file(&self, path: &Utf8Path, _text: &str, _default_id: &str) -> ModuleId {
        let name = path.as_str().trim_end_matches(".rb").to_owned();

        // `spec/` and `test/` require the code under test by design.
        let is_test = path
            .as_str()
            .split('/')
            .any(|segment| segment == "spec" || segment == "test" || segment == "features")
            || path
                .file_name()
                .is_some_and(|file| file.ends_with("_spec.rb") || file.ends_with("_test.rb"));

        if is_test {
            ModuleId::external_test(name)
        } else {
            ModuleId::module(name)
        }
    }

    fn resolve_import(
        &self,
        import: &Import,
        from: &Utf8Path,
        ctx: &ProjectContext<'_>,
    ) -> ImportTarget {
        let raw = import.raw.as_str();
        if raw.is_empty() {
            return ImportTarget::Unresolved {
                reason: "empty require".to_owned(),
            };
        }

        // `require_relative` is unambiguous: a path, relative to this file. It is
        // marked as a path reference at extraction so the two cannot be confused.
        if import.form == tropism_core::provider::ImportForm::PathReference {
            return match normalize_relative(from, raw) {
                Some(module) if ctx.known_modules.contains(&module) => {
                    ImportTarget::Internal(module)
                }
                Some(module) => ImportTarget::Unresolved {
                    reason: format!("`{raw}` resolves to `{module}`, which is not in this project"),
                },
                None => ImportTarget::Unresolved {
                    reason: format!("`{raw}` reaches above the project root"),
                },
            };
        }

        // A plain `require` searches the load path, which for an application is
        // `lib/` and the project root. Matching against the files actually present
        // is what distinguishes an internal require from a gem.
        if let Some(module) = load_path_target(raw, ctx) {
            return ImportTarget::Internal(module);
        }

        if self.is_stdlib(raw) {
            return ImportTarget::Stdlib;
        }

        // Ruby's convention is that `require "foo/bar"` comes from gem `foo-bar` or
        // gem `foo`. Both are tried against the manifest, longest first, before
        // anything is guessed.
        let candidates = gem_candidates(raw);
        for candidate in &candidates {
            if let Some(dep) = ctx.declared.iter().find(|dep| &dep.name == candidate) {
                return ImportTarget::External(dep.name.clone());
            }
            if let Some(sibling) = ctx.sibling_packages.iter().find(|name| *name == candidate) {
                return ImportTarget::External(sibling.clone());
            }
        }

        if let Some((_, gem)) = REQUIRE_TO_GEM
            .iter()
            .find(|(prefix, _)| *prefix == top_segment(raw))
        {
            return ImportTarget::External((*gem).to_owned());
        }

        // Undeclared: the top segment is the gem in the overwhelming majority of
        // cases, and naming it is what makes the missing-dep finding actionable.
        ImportTarget::External(top_segment(raw).to_owned())
    }

    fn is_stdlib(&self, module: &str) -> bool {
        STDLIB.contains(&top_segment(module))
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &RubyGemsOps
    }
}

fn top_segment(require: &str) -> &str {
    require.split('/').next().unwrap_or(require)
}

/// Gem names a `require` path could plausibly come from, longest first.
///
/// `require "faraday/retry"` is either the `faraday-retry` gem or a file inside
/// `faraday`. Both are real, so both are offered and the manifest decides.
fn gem_candidates(require: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    if require.contains('/') {
        candidates.push(require.replace('/', "-"));
    }
    candidates.push(top_segment(require).to_owned());
    candidates
}

/// Where a `require` lands, if it lands inside this project.
///
/// The load path of an application is `lib/` plus the project root, which is what
/// `bundler/setup` and every gemspec set up. Both are tried, and a hit only counts
/// when the file is one tropism actually walked.
fn load_path_target(require: &str, ctx: &ProjectContext<'_>) -> Option<String> {
    let root = ctx.project.root.as_str();
    for prefix in ["lib/", ""] {
        // Joined as a string rather than with `Utf8Path::join`, which inserts the
        // platform separator — `\` on Windows — while module identity is always
        // `/`. Mixing the two silently turned an internal require into a gem.
        let candidate = format!("{root}/{prefix}{require}");
        let module = candidate.trim_start_matches('/').to_owned();
        if ctx.known_modules.contains(&module) {
            return Some(module);
        }
    }
    None
}

/// `require_relative "../models/order"` from `lib/shop/api.rb` → `lib/models/order`.
fn normalize_relative(from: &Utf8Path, raw: &str) -> Option<String> {
    let base = from.parent().unwrap_or(Utf8Path::new(""));
    let mut parts: Vec<&str> = base
        .as_str()
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();

    for segment in raw.split('/') {
        match segment {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }

    let joined = parts.join("/");
    let trimmed = joined.trim_end_matches(".rb");
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

// --- Gemfile -----------------------------------------------------------------

/// Parses the declarative subset of a `Gemfile`.
///
/// Walks the Ruby syntax tree for `gem` calls, tracking the `group` blocks they sit
/// inside. Anything else — a conditional, a loop, an interpolated name — contributes
/// nothing rather than a guess, which is the rule for every manifest that is code.
fn parse_gemfile(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Ruby grammar failed: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut deps = Vec::new();
    collect_gems(tree.root_node(), text.as_bytes(), path, None, &mut deps);

    deps.sort_by(|a, b| a.name.cmp(&b.name));
    deps.dedup_by(|a, b| a.name == b.name);

    Ok(Manifest {
        deps,
        // A Gemfile names no gem of its own; a `.gemspec` does, and is not a
        // manifest tropism claims.
        package_name: None,
    })
}

fn collect_gems(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    path: &Utf8Path,
    group: Option<&str>,
    out: &mut Vec<DeclaredDep>,
) {
    if node.kind() == "call" {
        let method = node
            .child_by_field_name("method")
            .and_then(|method| method.utf8_text(source).ok())
            .unwrap_or_default();

        match method {
            "gem" => {
                if let Some(name) = first_string_argument(node, source) {
                    let requirement = node
                        .child_by_field_name("arguments")
                        .and_then(|args| {
                            let mut cursor = args.walk();
                            args.named_children(&mut cursor)
                                .filter(|child| child.kind() == "string")
                                .nth(1)
                                .and_then(|node| string_value(node, source))
                        })
                        .unwrap_or_default();

                    out.push(DeclaredDep {
                        name,
                        requirement,
                        kind: group_kind(group),
                        declared_at: Provenance::new(
                            path,
                            Some(node.start_position().row as u32 + 1),
                        ),
                    });
                }
                return;
            }
            "group" => {
                // The group's name applies to every `gem` inside its block.
                let name = node
                    .child_by_field_name("arguments")
                    .and_then(|args| args.utf8_text(source).ok())
                    .unwrap_or_default()
                    .to_owned();
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    collect_gems(child, source, path, Some(&name), out);
                }
                return;
            }
            _ => {}
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_gems(child, source, path, group, out);
    }
}

/// `group :development, :test` → Dev.
///
/// A gem in *any* non-production group is not expected to be required by shipped
/// code, so it is Dev even when the group list also names production.
fn group_kind(group: Option<&str>) -> DepKind {
    match group {
        Some(names) if names.contains("development") || names.contains("test") => DepKind::Dev,
        _ => DepKind::Runtime,
    }
}

fn first_string_argument(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let arguments = node.child_by_field_name("arguments")?;
    let mut cursor = arguments.walk();
    arguments
        .named_children(&mut cursor)
        .find(|child| child.kind() == "string")
        .and_then(|node| string_value(node, source))
}

/// The literal content of a string node, or `None` when it is interpolated.
///
/// `gem "rails-#{suffix}"` names no gem tropism can know, and inventing one would put
/// a package that does not exist into a report.
fn string_value(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<tree_sitter::Node<'_>> = node.named_children(&mut cursor).collect();
    if children.iter().any(|child| child.kind() == "interpolation") {
        return None;
    }
    match children.first() {
        Some(content) if content.kind() == "string_content" => {
            content.utf8_text(source).ok().map(str::to_owned)
        }
        // An empty string literal has no content child.
        None => None,
        _ => None,
    }
}

// --- Gemfile.lock ------------------------------------------------------------

/// Parses `Gemfile.lock`.
///
/// The format is indentation-structured rather than nested: under `specs:`, four
/// spaces is a resolved gem and six spaces is one of its dependencies. Bundler
/// resolves flat, so a name identifies a gem uniquely and edges need no
/// disambiguation — unlike npm, and unlike a forked `uv.lock`.
fn parse_gemfile_lock(path: &Utf8Path, text: &str) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
    let mut resolved: Vec<ResolvedDep> = Vec::new();
    let mut in_specs = false;

    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        // A section header sits at column zero. `specs:` is nested inside one.
        if !raw.starts_with(' ') {
            in_specs = false;
            continue;
        }
        if trimmed == "specs:" {
            in_specs = true;
            continue;
        }
        if !in_specs {
            continue;
        }

        let indent = raw.len() - raw.trim_start().len();
        let Some((name, version)) = split_spec(trimmed) else {
            continue;
        };

        if indent <= 4 {
            resolved.push(ResolvedDep {
                key: name.clone(),
                name,
                version: version.unwrap_or_default(),
                dependencies: Vec::new(),
            });
        } else if let Some(current) = resolved.last_mut() {
            current.dependencies.push(name);
        }
    }

    if resolved.is_empty() {
        anyhow::bail!("{path}: no resolved gems found under `specs:`");
    }

    // Edges may name a gem that is not itself locked — a platform-specific
    // dependency, say. Dropping those keeps every edge pointing at a real node.
    let known: std::collections::BTreeSet<String> =
        resolved.iter().map(|dep| dep.key.clone()).collect();
    for dep in &mut resolved {
        dep.dependencies.retain(|name| known.contains(name));
        dep.dependencies.sort();
        dep.dependencies.dedup();
    }

    Ok(Some(resolved))
}

/// `rack (2.2.8)` → `("rack", Some("2.2.8"))`; `rack (>= 2.2.4)` → `("rack", None)`,
/// because a constraint on a dependency line is a requirement, not a resolution.
fn split_spec(line: &str) -> Option<(String, Option<String>)> {
    let (name, rest) = match line.split_once(" (") {
        Some((name, rest)) => (name, Some(rest.trim_end_matches(')'))),
        None => (line, None),
    };
    let name = name.trim();
    if name.is_empty() || name.contains(' ') {
        return None;
    }
    let version = rest.filter(|value| {
        value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_digit())
    });
    Some((name.to_owned(), version.map(str::to_owned)))
}

// --- import extraction -------------------------------------------------------

/// Extracts `require` and `require_relative` from one Ruby file.
///
/// The two are distinguished by [`ImportForm`]: a `require_relative` is a path and
/// resolves against the file, a `require` is a load-path lookup that may name a
/// gem. Collapsing them would make every relative require look like a missing gem.
fn extract_ruby_requires(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_ruby::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Ruby grammar failed: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect_requires(tree.root_node(), text.as_bytes(), &mut imports);
    Ok(imports)
}

fn collect_requires(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    if node.kind() == "call" {
        let method = node
            .child_by_field_name("method")
            .and_then(|method| method.utf8_text(source).ok())
            .unwrap_or_default();

        if matches!(method, "require" | "require_relative") {
            if let Some(target) = first_string_argument(node, source) {
                let line = node.start_position().row as u32 + 1;
                out.push(if method == "require_relative" {
                    Import::path_reference(target, line)
                } else {
                    Import::statement(target, line)
                });
            }
            return;
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_requires(child, source, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;
    use tropism_core::model::Project;
    use tropism_core::provider::ImportForm;

    fn requires(source: &str) -> Vec<(String, ImportForm)> {
        extract_ruby_requires(Utf8Path::new("app.rb"), source)
            .unwrap()
            .into_iter()
            .map(|import| (import.raw, import.form))
            .collect()
    }

    fn gemfile(text: &str) -> Manifest {
        parse_gemfile(Utf8Path::new("Gemfile"), text).unwrap()
    }

    // --- Gemfile ----------------------------------------------------------

    #[test]
    fn parses_gems_and_their_requirements() {
        let manifest =
            gemfile("source \"https://rubygems.org\"\n\ngem \"rails\", \"~> 7.1\"\ngem \"pg\"\n");
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["pg", "rails"]);
        assert_eq!(manifest.deps[1].requirement, "~> 7.1");
    }

    #[test]
    fn gems_inside_a_development_group_are_dev_dependencies() {
        let manifest =
            gemfile("gem \"rails\"\n\ngroup :development, :test do\n  gem \"rspec\"\nend\n");
        let rspec = manifest.deps.iter().find(|d| d.name == "rspec").unwrap();
        let rails = manifest.deps.iter().find(|d| d.name == "rails").unwrap();
        assert_eq!(rspec.kind, DepKind::Dev);
        assert_eq!(rails.kind, DepKind::Runtime);
    }

    /// The Gemfile is a program, and `gem "rails-#{variant}"` names no gem that can
    /// be known without running it.
    #[test]
    fn an_interpolated_gem_name_is_skipped_rather_than_guessed() {
        let manifest = gemfile("suffix = \"api\"\ngem \"rails-#{suffix}\"\ngem \"pg\"\n");
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["pg"]);
    }

    #[test]
    fn records_the_declaring_line_for_evidence() {
        let manifest = gemfile("source \"https://rubygems.org\"\n\ngem \"pg\"\n");
        assert_eq!(manifest.deps[0].declared_at.line, Some(3));
    }

    #[test]
    fn source_and_ruby_directives_are_not_dependencies() {
        let manifest = gemfile("source \"https://rubygems.org\"\nruby \"3.3.0\"\n");
        assert!(manifest.deps.is_empty());
    }

    // --- Gemfile.lock -----------------------------------------------------

    fn lock(text: &str) -> Vec<ResolvedDep> {
        parse_gemfile_lock(Utf8Path::new("Gemfile.lock"), text)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn parses_resolved_gems_and_their_edges() {
        let resolved = lock(concat!(
            "GEM\n",
            "  remote: https://rubygems.org/\n",
            "  specs:\n",
            "    actionpack (7.1.3)\n",
            "      rack (>= 2.2.4)\n",
            "    rack (2.2.8)\n",
            "\n",
            "PLATFORMS\n",
            "  ruby\n",
            "\n",
            "DEPENDENCIES\n",
            "  rails (~> 7.1)\n",
        ));

        assert_eq!(resolved.len(), 2, "only the two under `specs:`");
        assert_eq!(resolved[0].name, "actionpack");
        assert_eq!(resolved[0].version, "7.1.3");
        assert_eq!(resolved[0].dependencies, vec!["rack"]);
        assert_eq!(resolved[1].version, "2.2.8");
    }

    /// `rack (>= 2.2.4)` on a dependency line is a requirement, not a resolution,
    /// and must not be recorded as the version rack resolved to.
    #[test]
    fn a_constraint_is_not_mistaken_for_a_resolved_version() {
        assert_eq!(
            split_spec("rack (>= 2.2.4)"),
            Some(("rack".to_owned(), None))
        );
        assert_eq!(
            split_spec("rack (2.2.8)"),
            Some(("rack".to_owned(), Some("2.2.8".to_owned())))
        );
    }

    #[test]
    fn the_dependencies_section_is_not_read_as_resolved_gems() {
        let resolved = lock("GEM\n  specs:\n    rack (2.2.8)\n\nDEPENDENCIES\n  rails (~> 7.1)\n");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name, "rack");
    }

    #[test]
    fn a_lockfile_with_no_specs_is_an_error() {
        assert!(parse_gemfile_lock(Utf8Path::new("Gemfile.lock"), "PLATFORMS\n  ruby\n").is_err());
    }

    // --- import extraction ------------------------------------------------

    #[test]
    fn extracts_require_and_require_relative_distinctly() {
        assert_eq!(
            requires("require \"json\"\nrequire_relative \"order\"\n"),
            vec![
                ("json".to_owned(), ImportForm::Statement),
                ("order".to_owned(), ImportForm::PathReference),
            ]
        );
    }

    /// The reason for a grammar instead of a regex.
    #[test]
    fn ignores_requires_in_strings_and_comments() {
        let source = concat!(
            "require \"json\"\n",
            "# require \"fake_commented\"\n",
            "s = \"require 'fake_quoted'\"\n",
            "doc = <<~TEXT\n  require \"fake_heredoc\"\nTEXT\n",
        );
        assert_eq!(
            requires(source)
                .into_iter()
                .map(|(raw, _)| raw)
                .collect::<Vec<_>>(),
            vec!["json"]
        );
    }

    #[test]
    fn finds_requires_nested_in_blocks_and_classes() {
        let source = "class Foo\n  def bar\n    require \"csv\"\n  end\nend\n";
        assert_eq!(requires(source).len(), 1);
    }

    #[test]
    fn reports_accurate_line_numbers() {
        let imports = extract_ruby_requires(
            Utf8Path::new("a.rb"),
            "require \"json\"\n\nrequire \"set\"\n",
        )
        .unwrap();
        assert_eq!(imports[0].line, 1);
        assert_eq!(imports[1].line, 3);
    }

    // --- module identity --------------------------------------------------

    #[test]
    fn a_module_is_the_file_path_without_its_extension() {
        let id =
            RubyProvider.module_id_for_file(Utf8Path::new("lib/shop/order.rb"), "", "lib/shop");
        assert_eq!(id.name, "lib/shop/order");
    }

    #[test]
    fn spec_files_are_external_test_modules() {
        for file in ["spec/order_spec.rb", "test/order_test.rb"] {
            assert!(
                RubyProvider
                    .module_id_for_file(Utf8Path::new(file), "", "spec")
                    .kind
                    .is_test(),
                "{file}"
            );
        }
    }

    // --- resolution -------------------------------------------------------

    fn resolve(
        file: &str,
        raw: &str,
        form: ImportForm,
        declared: &[&str],
        modules: &[&str],
    ) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::from("app"),
            language: Language::Ruby,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: String::new(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("Gemfile", Some(1)),
            })
            .collect();
        let known: BTreeSet<String> = modules.iter().map(|m| (*m).to_owned()).collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: None,
            declared: &deps,
            sibling_packages: &[],
            known_modules: &known,
            source_files: &[],
        };
        let import = match form {
            ImportForm::Statement => Import::statement(raw, 1),
            ImportForm::PathReference => Import::path_reference(raw, 1),
        };
        RubyProvider.resolve_import(&import, Utf8Path::new(file), &ctx)
    }

    #[test]
    fn resolves_stdlib_requires() {
        assert_eq!(
            resolve("app/a.rb", "json", ImportForm::Statement, &[], &[]),
            ImportTarget::Stdlib
        );
        assert_eq!(
            resolve("app/a.rb", "net/http", ImportForm::Statement, &[], &[]),
            ImportTarget::Stdlib
        );
    }

    #[test]
    fn resolves_a_require_relative_against_the_current_file() {
        assert_eq!(
            resolve(
                "app/lib/shop/api.rb",
                "../models/order",
                ImportForm::PathReference,
                &[],
                &["app/lib/models/order"],
            ),
            ImportTarget::Internal("app/lib/models/order".to_owned())
        );
    }

    /// The load-path candidate must be built with `/` on every platform.
    /// `Utf8Path::join` inserts `\` on Windows, and module identity never does, so
    /// this resolved to a gem called `shop` there and only there.
    #[test]
    fn load_path_candidates_use_forward_slashes_on_every_platform() {
        let project = Project {
            root: Utf8PathBuf::from("app"),
            language: Language::Ruby,
            manifests: vec![],
            lockfile: None,
        };
        let known: BTreeSet<String> = ["app/lib/shop/order".to_owned()].into_iter().collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: None,
            declared: &[],
            sibling_packages: &[],
            known_modules: &known,
            source_files: &[],
        };
        assert_eq!(
            load_path_target("shop/order", &ctx).as_deref(),
            Some("app/lib/shop/order")
        );
    }

    /// A plain `require` searches the load path, which for an application is `lib/`.
    #[test]
    fn resolves_a_require_through_the_lib_load_path() {
        assert_eq!(
            resolve(
                "app/lib/shop.rb",
                "shop/order",
                ImportForm::Statement,
                &[],
                &["app/lib/shop/order"],
            ),
            ImportTarget::Internal("app/lib/shop/order".to_owned())
        );
    }

    #[test]
    fn resolves_a_declared_gem() {
        assert_eq!(
            resolve("app/a.rb", "pg", ImportForm::Statement, &["pg"], &[]),
            ImportTarget::External("pg".to_owned())
        );
    }

    /// `require "faraday/retry"` is either a file inside `faraday` or the
    /// `faraday-retry` gem, and the manifest is what decides.
    #[test]
    fn a_nested_require_prefers_the_declared_hyphenated_gem() {
        assert_eq!(
            resolve(
                "app/a.rb",
                "faraday/retry",
                ImportForm::Statement,
                &["faraday-retry"],
                &[]
            ),
            ImportTarget::External("faraday-retry".to_owned())
        );
        assert_eq!(
            resolve(
                "app/a.rb",
                "faraday/retry",
                ImportForm::Statement,
                &["faraday"],
                &[]
            ),
            ImportTarget::External("faraday".to_owned())
        );
    }

    /// The Rails family: the file is snake_case, the gem is not.
    #[test]
    fn resolves_the_rails_family_to_its_gem() {
        assert_eq!(
            resolve(
                "app/a.rb",
                "active_support/all",
                ImportForm::Statement,
                &[],
                &[]
            ),
            ImportTarget::External("activesupport".to_owned())
        );
    }

    #[test]
    fn an_undeclared_gem_is_external_so_it_is_reported_missing() {
        assert_eq!(
            resolve("app/a.rb", "nokogiri", ImportForm::Statement, &[], &[]),
            ImportTarget::External("nokogiri".to_owned())
        );
    }

    #[test]
    fn a_require_relative_leaving_the_project_is_unresolved() {
        assert!(matches!(
            resolve(
                "app/a.rb",
                "../../outside",
                ImportForm::PathReference,
                &[],
                &[]
            ),
            ImportTarget::Unresolved { .. }
        ));
    }
}
