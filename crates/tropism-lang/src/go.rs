//! Go provider.
//!
//! First implementation by design: Go's import→package mapping is the simplest of
//! the ten — the declared module path is a literal prefix of every internal import,
//! and external ones resolve by longest declared prefix. No exception table needed.
//! See `design/07-open-questions.md`, build order.

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};

pub struct GoProvider;

/// Go's standard library top-level packages.
///
/// The structural rule does most of the work: an import whose first path segment
/// contains a dot is a domain, so third-party. This list disambiguates the rest.
const STDLIB_ROOTS: &[&str] = &[
    "archive",
    "arena",
    "bufio",
    "builtin",
    "bytes",
    "cmp",
    "compress",
    "container",
    "context",
    "crypto",
    "database",
    "debug",
    "embed",
    "encoding",
    "errors",
    "expvar",
    "flag",
    "fmt",
    "go",
    "hash",
    "html",
    "image",
    "index",
    "io",
    "iter",
    "log",
    "maps",
    "math",
    "mime",
    "net",
    "os",
    "path",
    "plugin",
    "reflect",
    "regexp",
    "runtime",
    "slices",
    "sort",
    "strconv",
    "strings",
    "structs",
    "sync",
    "syscall",
    "testing",
    "text",
    "time",
    "unicode",
    "unique",
    "unsafe",
    "weak",
];

struct GoVersionOps;

impl VersionOps for GoVersionOps {
    /// Not implemented: Go's resolved tree is unavailable without running the
    /// resolver, so nothing compares versions yet. Returning `None` is honest;
    /// a lexical comparison would be wrong.
    fn compare(&self, _a: &str, _b: &str) -> Option<std::cmp::Ordering> {
        None
    }

    fn satisfies(&self, _version: &str, _requirement: &str) -> Option<bool> {
        None
    }
}

impl LanguageProvider for GoProvider {
    fn language(&self) -> Language {
        Language::Go
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["go.mod"]
    }

    fn lockfile_names(&self) -> &'static [&'static str] {
        &["go.sum"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn parse_manifest(&self, _path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        Ok(parse_go_mod(text))
    }

    /// `go.sum` is deliberately not treated as a resolved tree — see
    /// [`Self::resolved_tree_note`].
    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        _text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        Ok(None)
    }

    fn resolved_tree_note(&self) -> Option<&'static str> {
        Some(
            "go.sum records hashes for the whole module graph, not the versions MVS \
             selected, and carries no edges; a resolved tree needs the Go resolver",
        )
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_go_imports(path, text)
    }

    /// Classifies a Go file into the three-way split `ModuleKind` describes.
    ///
    /// Both halves of the rule were forced by false positives on real code:
    ///
    /// * Treating `_test.go` files as part of their package produced a 15-module
    ///   phantom cycle in Prometheus — a package's test files are not compiled in
    ///   when it is built as a dependency of another package's tests.
    /// * Treating all `_test.go` files as one test module then flagged `package
    ///   doc_test` importing `doc` in Cobra, Zerolog, and Prometheus. That is the
    ///   designed use of an external test package, not a defect.
    fn module_id_for_file(&self, path: &Utf8Path, text: &str, default_id: &str) -> ModuleId {
        let is_test_file = path
            .file_name()
            .is_some_and(|name| name.ends_with("_test.go"));
        if !is_test_file {
            return ModuleId::module(default_id);
        }

        match package_clause(text) {
            Some(name) if name.ends_with("_test") => ModuleId::external_test(default_id),
            _ => ModuleId::internal_test(default_id),
        }
    }

    fn resolve_import(
        &self,
        import: &Import,
        _from: &Utf8Path,
        ctx: &ProjectContext<'_>,
    ) -> ImportTarget {
        let path = import.raw.as_str();

        // 1. Internal: the module path is a literal prefix of the import.
        if let Some(module) = ctx.package_name
            && let Some(rest) = strip_module_prefix(path, module)
        {
            return ImportTarget::Internal(if rest.is_empty() {
                ".".to_owned()
            } else {
                rest
            });
        }

        // 2. Stdlib: no dot in the first segment.
        if self.is_stdlib(path) {
            return ImportTarget::Stdlib;
        }

        // 3. External by longest declared prefix — authoritative when it matches.
        let best = ctx
            .declared
            .iter()
            .filter(|dep| strip_module_prefix(path, &dep.name).is_some())
            .max_by_key(|dep| dep.name.len());
        if let Some(dep) = best {
            return ImportTarget::External(dep.name.clone());
        }

        // 4. External by host structure. Only applied to shapes where the module
        //    boundary is unambiguous; anything else stays Unresolved rather than
        //    becoming a confident wrong "missing dependency".
        match guess_module_path(path) {
            Some(module) => ImportTarget::External(module),
            None => ImportTarget::Unresolved {
                reason: format!("`{path}` matches no declared module and no known host layout"),
            },
        }
    }

    fn is_stdlib(&self, module: &str) -> bool {
        let root = module.split('/').next().unwrap_or(module);
        !root.contains('.') && STDLIB_ROOTS.contains(&root)
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &GoVersionOps
    }
}

/// Returns the remainder of `path` after `prefix`, if `prefix` is a *path-segment*
/// prefix. Guards against `example.com/apiserver` matching module `example.com/api`.
fn strip_module_prefix(path: &str, prefix: &str) -> Option<String> {
    if path == prefix {
        return Some(String::new());
    }
    path.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .map(str::to_owned)
}

/// The module path for a well-known host layout.
///
/// Deliberately conservative. A wrong guess here becomes a confident false "missing
/// dependency", which is worse than an admitted gap.
fn guess_module_path(path: &str) -> Option<String> {
    let segments: Vec<&str> = path.split('/').collect();
    let host = *segments.first()?;

    let take =
        match host {
            // Forges: host/owner/repo.
            "github.com" | "gitlab.com" | "bitbucket.org" | "codeberg.org" | "sr.ht" => 3,
            // golang.org/x/sync, google.golang.org/grpc, gopkg.in/yaml.v3.
            "golang.org" | "google.golang.org" | "k8s.io" | "sigs.k8s.io" => {
                if host == "golang.org" { 3 } else { 2 }
            }
            "gopkg.in" => 2,
            _ => return None,
        };

    (segments.len() >= take).then(|| segments[..take].join("/"))
}

/// Parses `go.mod`.
///
/// A hand-written line parser: the format is line-oriented and small, and the
/// alternative would be invoking `go`, which the project constraint forbids.
fn parse_go_mod(text: &str) -> Manifest {
    let mut manifest = Manifest::default();
    let mut in_require_block = false;

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index as u32 + 1;
        let (content, comment) = split_comment(raw_line);
        let content = content.trim();

        if content.is_empty() {
            continue;
        }

        if in_require_block {
            if content == ")" {
                in_require_block = false;
            } else if let Some(dep) = parse_require(content, comment, line_number) {
                manifest.deps.push(dep);
            }
            continue;
        }

        if let Some(module) = content.strip_prefix("module ") {
            manifest.package_name = Some(module.trim().to_owned());
        } else if content == "require (" {
            in_require_block = true;
        } else if let Some(rest) = content.strip_prefix("require ")
            && let Some(dep) = parse_require(rest.trim(), comment, line_number)
        {
            manifest.deps.push(dep);
        }
        // `go`, `toolchain`, `exclude`, `retract`, and `replace` are ignored.
        // `replace` is a real gap: it can redirect or drop a requirement, and
        // ignoring it can produce a wrong unused-dependency finding.
    }

    manifest
}

fn split_comment(line: &str) -> (&str, &str) {
    match line.find("//") {
        Some(at) => (&line[..at], &line[at + 2..]),
        None => (line, ""),
    }
}

fn parse_require(content: &str, comment: &str, line: u32) -> Option<DeclaredDep> {
    let mut parts = content.split_whitespace();
    let name = parts.next()?.to_owned();
    let requirement = parts.next().unwrap_or_default().to_owned();

    // `// indirect` means "not directly imported" — flagging it as unused would be
    // a false positive on every well-formed go.mod in existence.
    let kind = if comment.trim() == "indirect" {
        DepKind::Indirect
    } else {
        DepKind::Runtime
    };

    Some(DeclaredDep {
        name,
        requirement,
        kind,
        declared_at: Provenance::new("go.mod", Some(line)),
    })
}

/// The `package` clause: the first statement in a Go file, ignoring comments.
fn package_clause(text: &str) -> Option<&str> {
    let mut in_block_comment = false;

    for raw in text.lines() {
        let line = raw.trim();

        if in_block_comment {
            match line.find("*/") {
                Some(at) if line[at + 2..].trim().is_empty() => {
                    in_block_comment = false;
                    continue;
                }
                Some(_) => in_block_comment = false,
                None => continue,
            }
        }

        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with("/*") && !line.contains("*/") {
            in_block_comment = true;
            continue;
        }

        // The package clause is the first real statement, so anything else here
        // means the file has none.
        return line
            .strip_prefix("package ")
            .map(|name| name.split_whitespace().next().unwrap_or(name));
    }

    None
}

/// Extracts every import from one Go file using tree-sitter.
///
/// A grammar rather than a regex: imports are syntax, and regex over source is
/// defeated by strings, comments, and multi-line forms in every language.
fn extract_go_imports(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Go grammar failed: {error}"))?;

    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect_import_specs(tree.root_node(), text.as_bytes(), &mut imports);
    Ok(imports)
}

fn collect_import_specs(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    if node.kind() == "import_spec" {
        if let Some(path_node) = node.child_by_field_name("path")
            && let Ok(raw) = path_node.utf8_text(source)
        {
            // A blank import (`_ "embed"`) is a real dependency: it exists for its
            // side effects. Counting it as unused would be wrong.
            out.push(
                Import::statement(
                    raw.trim_matches('"').to_owned(),
                    path_node.start_position().row as u32 + 1,
                )
                .type_only(false),
            );
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_import_specs(child, source, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tropism_core::model::Project;

    fn extract(source: &str) -> Vec<Import> {
        extract_go_imports(Utf8Path::new("test.go"), source).unwrap()
    }

    fn paths(source: &str) -> Vec<String> {
        extract(source)
            .into_iter()
            .map(|import| import.raw)
            .collect()
    }

    // --- go.mod parsing ---------------------------------------------------

    #[test]
    fn parses_the_module_path() {
        let manifest = parse_go_mod("module example.com/api\n\ngo 1.24\n");
        assert_eq!(manifest.package_name.as_deref(), Some("example.com/api"));
        assert!(manifest.deps.is_empty());
    }

    #[test]
    fn parses_a_require_block() {
        let manifest = parse_go_mod(
            "module example.com/api\n\nrequire (\n\tgithub.com/spf13/cobra v1.8.0\n\tgolang.org/x/sync v0.7.0\n)\n",
        );
        assert_eq!(manifest.deps.len(), 2);
        assert_eq!(manifest.deps[0].name, "github.com/spf13/cobra");
        assert_eq!(manifest.deps[0].requirement, "v1.8.0");
        assert_eq!(manifest.deps[1].name, "golang.org/x/sync");
    }

    #[test]
    fn parses_a_single_line_require() {
        let manifest = parse_go_mod("module m\n\nrequire github.com/pkg/errors v0.9.1\n");
        assert_eq!(manifest.deps.len(), 1);
        assert_eq!(manifest.deps[0].name, "github.com/pkg/errors");
    }

    #[test]
    fn marks_indirect_requirements() {
        let manifest = parse_go_mod(
            "module m\n\nrequire (\n\tgithub.com/a/b v1.0.0\n\tgithub.com/c/d v2.0.0 // indirect\n)\n",
        );
        assert_eq!(manifest.deps[0].kind, DepKind::Runtime);
        assert_eq!(manifest.deps[1].kind, DepKind::Indirect);
    }

    #[test]
    fn records_the_declaring_line_for_evidence() {
        let manifest = parse_go_mod("module m\n\nrequire (\n\tgithub.com/a/b v1.0.0\n)\n");
        assert_eq!(manifest.deps[0].declared_at.line, Some(4));
    }

    #[test]
    fn ignores_directives_that_are_not_requirements() {
        let manifest = parse_go_mod(
            "module m\ngo 1.24\ntoolchain go1.24.0\nexclude github.com/x/y v1.0.0\nretract v0.1.0\n",
        );
        assert!(manifest.deps.is_empty());
    }

    #[test]
    fn tolerates_an_empty_manifest() {
        let manifest = parse_go_mod("");
        assert_eq!(manifest.package_name, None);
        assert!(manifest.deps.is_empty());
    }

    // --- import extraction ------------------------------------------------

    #[test]
    fn extracts_a_single_import() {
        assert_eq!(paths("package main\n\nimport \"fmt\"\n"), vec!["fmt"]);
    }

    #[test]
    fn extracts_a_grouped_import_block() {
        let source = "package main\n\nimport (\n\t\"fmt\"\n\t\"net/http\"\n)\n";
        assert_eq!(paths(source), vec!["fmt", "net/http"]);
    }

    #[test]
    fn extracts_aliased_blank_and_dot_imports() {
        let source = concat!(
            "package main\n\n",
            "import (\n",
            "\tm \"math\"\n",
            "\t_ \"embed\"\n",
            "\t. \"strings\"\n",
            ")\n",
        );
        assert_eq!(paths(source), vec!["math", "embed", "strings"]);
    }

    /// The reason for a grammar instead of a regex.
    #[test]
    fn ignores_import_like_text_in_strings_and_comments() {
        let source = concat!(
            "package main\n\n",
            "import \"fmt\"\n\n",
            "// import \"github.com/fake/commented\"\n",
            "var s = `import \"github.com/fake/raw\"`\n",
            "var t = \"import \\\"github.com/fake/quoted\\\"\"\n",
        );
        assert_eq!(paths(source), vec!["fmt"], "only the real import counts");
    }

    #[test]
    fn reports_accurate_line_numbers() {
        let source = "package main\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n";
        let imports = extract(source);
        assert_eq!(imports[0].line, 4);
        assert_eq!(imports[1].line, 5);
    }

    #[test]
    fn a_file_with_no_imports_yields_nothing() {
        assert!(paths("package main\n\nfunc main() {}\n").is_empty());
    }

    /// tree-sitter recovers from errors rather than failing, so a broken file
    /// still yields the imports it could parse.
    #[test]
    fn malformed_source_does_not_error() {
        let imports = extract("package main\n\nimport \"fmt\"\n\nfunc broken( {{{\n");
        assert_eq!(imports.len(), 1);
    }

    // --- resolution -------------------------------------------------------

    fn resolve(module: &str, declared: &[&str], path: &str) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::from("svc"),
            language: Language::Go,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: "v1.0.0".to_owned(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("go.mod", Some(1)),
            })
            .collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: Some(module),
            declared: &deps,
            sibling_packages: &[],
            known_modules: &std::collections::BTreeSet::new(),
            source_files: &[],
        };
        let import = Import::statement(path.to_owned(), 1).type_only(false);
        GoProvider.resolve_import(&import, Utf8Path::new("svc/main.go"), &ctx)
    }

    #[test]
    fn resolves_an_internal_import_to_its_package_directory() {
        assert_eq!(
            resolve("example.com/api", &[], "example.com/api/store/db"),
            ImportTarget::Internal("store/db".to_owned())
        );
    }

    #[test]
    fn resolves_the_module_root_import_to_dot() {
        assert_eq!(
            resolve("example.com/api", &[], "example.com/api"),
            ImportTarget::Internal(".".to_owned())
        );
    }

    /// `example.com/apiserver` must not be mistaken for module `example.com/api`.
    #[test]
    fn a_prefix_only_matches_on_a_segment_boundary() {
        let target = resolve("example.com/api", &[], "example.com/apiserver/x");
        assert!(
            !matches!(target, ImportTarget::Internal(_)),
            "got {target:?}"
        );
    }

    #[test]
    fn resolves_stdlib_imports() {
        assert_eq!(resolve("m", &[], "fmt"), ImportTarget::Stdlib);
        assert_eq!(resolve("m", &[], "net/http"), ImportTarget::Stdlib);
        assert_eq!(resolve("m", &[], "encoding/json"), ImportTarget::Stdlib);
    }

    #[test]
    fn resolves_an_external_import_by_declared_prefix() {
        assert_eq!(
            resolve(
                "m",
                &["github.com/spf13/cobra"],
                "github.com/spf13/cobra/doc"
            ),
            ImportTarget::External("github.com/spf13/cobra".to_owned())
        );
    }

    #[test]
    fn the_longest_declared_prefix_wins() {
        assert_eq!(
            resolve(
                "m",
                &["github.com/a/b", "github.com/a/b/sub"],
                "github.com/a/b/sub/deep"
            ),
            ImportTarget::External("github.com/a/b/sub".to_owned())
        );
    }

    #[test]
    fn guesses_the_module_for_known_host_layouts() {
        assert_eq!(
            resolve("m", &[], "github.com/x/y/internal/z"),
            ImportTarget::External("github.com/x/y".to_owned())
        );
        assert_eq!(
            resolve("m", &[], "golang.org/x/sync/errgroup"),
            ImportTarget::External("golang.org/x/sync".to_owned())
        );
        assert_eq!(
            resolve("m", &[], "gopkg.in/yaml.v3"),
            ImportTarget::External("gopkg.in/yaml.v3".to_owned())
        );
    }

    /// An unknown host stays Unresolved rather than becoming a confident guess.
    #[test]
    fn an_unknown_host_layout_is_unresolved() {
        let target = resolve("m", &[], "example.org/some/deep/path");
        assert!(
            matches!(target, ImportTarget::Unresolved { .. }),
            "got {target:?}"
        );
    }

    // --- test-file classification ----------------------------------------

    fn classify(file: &str, source: &str) -> tropism_core::graph::ModuleId {
        GoProvider.module_id_for_file(Utf8Path::new(file), source, "api")
    }

    #[test]
    fn ordinary_files_are_normal_modules() {
        let id = classify("api/api.go", "package api\n");
        assert_eq!(id, tropism_core::graph::ModuleId::module("api"));
    }

    #[test]
    fn in_package_test_files_are_internal_test_modules() {
        let id = classify("api/api_test.go", "package api\n");
        assert_eq!(id, tropism_core::graph::ModuleId::internal_test("api"));
    }

    /// `package api_test` is a separate package by design, so it can import `api`
    /// without that being a cycle.
    #[test]
    fn external_test_packages_are_their_own_kind() {
        let id = classify("api/api_test.go", "package api_test\n");
        assert_eq!(id, tropism_core::graph::ModuleId::external_test("api"));
    }

    #[test]
    fn the_package_clause_is_found_past_comments_and_build_tags() {
        let source = "//go:build linux\n\n// Package api does things.\npackage api_test\n";
        assert_eq!(package_clause(source), Some("api_test"));
    }

    #[test]
    fn the_package_clause_is_found_past_a_block_comment_licence_header() {
        let source = "/*\nCopyright 2026.\nLicensed under Apache 2.0.\n*/\n\npackage api\n";
        assert_eq!(package_clause(source), Some("api"));
    }

    #[test]
    fn go_sum_is_not_treated_as_a_resolved_tree() {
        let parsed = GoProvider
            .parse_lockfile(Utf8Path::new("go.sum"), "anything")
            .unwrap();
        assert!(parsed.is_none());
        assert!(GoProvider.resolved_tree_note().unwrap().contains("go.sum"));
    }
}
