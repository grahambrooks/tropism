//! C++ provider.
//!
//! C++ has no package manager in the language, so this provider covers the two that
//! the ecosystem actually converged on — Conan and vcpkg — and both of their
//! manifest formats, including `conanfile.py`, which is a Python program and is
//! parsed with the Python grammar rather than executed.
//!
//! Two things are unlike every other language here.
//!
//! **A module is a component, not a file.** `include/shop/order.hpp` and
//! `src/order.cpp` are two files of one thing, and `#include "shop/order.hpp"`
//! names the header wherever it is on the include path. So the source-root prefix
//! is stripped and the extension dropped: both files become `shop/order`, and a
//! translation unit including its own header is a self-edge rather than a
//! two-module cycle.
//!
//! **There is no resolved tree anywhere.** `conan.lock` is a flat list of pinned
//! references, and vcpkg pins a registry baseline commit rather than a dependency
//! graph. Neither carries edges, so neither can answer a diamond question — the
//! same position as `go.sum`, `gradle.lockfile`, and `Package.resolved`.

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};

pub struct CppProvider;

/// Directory prefixes that are include-path roots rather than part of a name.
///
/// `#include "shop/order.hpp"` compiles because `include/` is on the include path;
/// the header's name never contains it. Stripping these is what lets a `.cpp` and
/// its `.hpp` be one module.
const SOURCE_ROOTS: &[&str] = &["include", "src", "source", "sources", "inc", "lib"];

/// C and POSIX headers that need no package. The C++ standard headers are
/// recognised structurally instead — see [`is_standard_header`].
const SYSTEM_HEADERS: &[&str] = &[
    "aio.h",
    "arpa",
    "assert.h",
    "complex.h",
    "ctype.h",
    "dirent.h",
    "dlfcn.h",
    "errno.h",
    "fcntl.h",
    "fenv.h",
    "float.h",
    "grp.h",
    "inttypes.h",
    "iso646.h",
    "libgen.h",
    "limits.h",
    "locale.h",
    "math.h",
    "net",
    "netinet",
    "poll.h",
    "pthread.h",
    "pwd.h",
    "sched.h",
    "semaphore.h",
    "setjmp.h",
    "signal.h",
    "stdalign.h",
    "stdarg.h",
    "stdatomic.h",
    "stdbool.h",
    "stddef.h",
    "stdint.h",
    "stdio.h",
    "stdlib.h",
    "string.h",
    "strings.h",
    "sys",
    "syslog.h",
    "termios.h",
    "tgmath.h",
    "threads.h",
    "time.h",
    "uchar.h",
    "unistd.h",
    "wchar.h",
    "wctype.h",
    "windows.h",
];

/// Include prefixes whose package is not the first path segment.
///
/// The first segment is right for `fmt/format.h`, `spdlog/spdlog.h`, and most of
/// the ecosystem, so this is only the residue. Conan and vcpkg disagree about some
/// of these names; where they do, the Conan reference is used, because a
/// `conanfile` is the more common manifest.
const INCLUDE_TO_PACKAGE: &[(&str, &str)] = &[
    ("absl", "abseil"),
    ("asio", "asio"),
    ("benchmark", "benchmark"),
    ("catch2", "catch2"),
    ("Eigen", "eigen"),
    ("google", "protobuf"),
    ("gmock", "gtest"),
    ("gsl", "ms-gsl"),
    ("nlohmann", "nlohmann_json"),
    ("rapidjson", "rapidjson"),
    ("tbb", "onetbb"),
    ("yaml-cpp", "yaml-cpp"),
];

struct ConanVersionOps;

impl VersionOps for ConanVersionOps {
    /// Conan and vcpkg both use dotted numeric versions in practice. `None` on
    /// anything else, so a wrong ordering never reaches a finding.
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

    fn satisfies(&self, _version: &str, _requirement: &str) -> Option<bool> {
        None
    }
}

impl LanguageProvider for CppProvider {
    fn language(&self) -> Language {
        Language::Cpp
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["conanfile.py", "conanfile.txt", "vcpkg.json"]
    }

    fn lockfile_names(&self) -> &'static [&'static str] {
        &["conan.lock", "vcpkg-configuration.json"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &[
            "cpp", "cc", "cxx", "c", "hpp", "hh", "hxx", "h", "ipp", "inl",
        ]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        match path.file_name() {
            Some("vcpkg.json") => parse_vcpkg(path, text),
            Some("conanfile.py") => parse_conanfile_py(path, text),
            _ => Ok(parse_conanfile_txt(path, text)),
        }
    }

    /// Neither ecosystem records a resolved tree — see [`Self::resolved_tree_note`].
    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        _text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        Ok(None)
    }

    fn resolved_tree_note(&self) -> Option<&'static str> {
        Some(
            "conan.lock is a flat list of pinned references and vcpkg pins a registry \
             baseline commit rather than a graph; neither carries dependency edges, so \
             a resolved tree needs the package manager",
        )
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_includes(path, text)
    }

    /// A C++ module is a *component*: the path with its include-path root stripped
    /// and its extension dropped, so `include/shop/order.hpp` and `src/order.cpp`
    /// are one module rather than two.
    ///
    /// Without this a translation unit including its own header would be an edge
    /// between two nodes, and every component in the project would appear in the
    /// graph twice.
    fn module_id_for_file(&self, path: &Utf8Path, _text: &str, _default_id: &str) -> ModuleId {
        let name = component_name(path);

        let is_test = path
            .as_str()
            .split('/')
            .any(|segment| matches!(segment, "test" | "tests" | "testing"))
            || path
                .file_stem()
                .is_some_and(|stem| stem.ends_with("_test") || stem.starts_with("test_"));

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
                reason: "empty include".to_owned(),
            };
        }

        // A quoted include searches the including file's directory first, then the
        // include path. Both are tried against the files tropism actually walked.
        if import.form == tropism_core::provider::ImportForm::PathReference
            && let Some(module) = beside_the_including_file(from, raw, ctx)
        {
            return ImportTarget::Internal(module);
        }

        // On the include path: `shop/order.hpp` names the component `shop/order`
        // wherever the header sits.
        let component = strip_extension(raw);
        if ctx.known_modules.contains(component) {
            return ImportTarget::Internal(component.to_owned());
        }
        if let Some(module) = ctx
            .known_modules
            .iter()
            .find(|module| module.ends_with(&format!("/{component}")))
        {
            return ImportTarget::Internal(module.clone());
        }

        if self.is_stdlib(raw) {
            return ImportTarget::Stdlib;
        }

        // The package is the first path segment for most of the ecosystem, and a
        // bare header (`zlib.h`, `sqlite3.h`) names the package in its stem.
        let candidate = package_candidate(raw);
        if let Some(dep) = ctx
            .declared
            .iter()
            .find(|dep| same_package(&dep.name, &candidate))
        {
            return ImportTarget::External(dep.name.clone());
        }
        if let Some(sibling) = ctx
            .sibling_packages
            .iter()
            .find(|name| same_package(name, &candidate))
        {
            return ImportTarget::External(sibling.clone());
        }
        if let Some((_, package)) = INCLUDE_TO_PACKAGE
            .iter()
            .find(|(prefix, _)| *prefix == first_segment(raw))
        {
            return ImportTarget::External((*package).to_owned());
        }

        ImportTarget::External(candidate)
    }

    fn is_stdlib(&self, header: &str) -> bool {
        is_standard_header(header)
            || SYSTEM_HEADERS.contains(&header)
            || SYSTEM_HEADERS.contains(&first_segment(header))
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &ConanVersionOps
    }
}

/// A C++ standard header has neither a directory nor an extension: `<vector>`,
/// `<string_view>`, `<cstdio>`. A structural rule rather than a list, which is what
/// keeps it correct as the standard library grows.
fn is_standard_header(header: &str) -> bool {
    !header.contains('/') && !header.contains('.') && !header.is_empty()
}

fn first_segment(header: &str) -> &str {
    header.split('/').next().unwrap_or(header)
}

fn strip_extension(path: &str) -> &str {
    match path.rsplit_once('.') {
        // Only a known source extension is an extension; `yaml-cpp/yaml.h` must not
        // lose anything else.
        Some((head, tail)) if tail.chars().all(|ch| ch.is_ascii_alphanumeric()) => head,
        _ => path,
    }
}

/// The package an include most likely comes from: the first path segment, or the
/// file stem when the header sits at the root of the include path.
fn package_candidate(header: &str) -> String {
    if header.contains('/') {
        first_segment(header).to_owned()
    } else {
        strip_extension(header).to_owned()
    }
}

/// Conan and vcpkg spell the same package with `-` or `_`, and case varies.
fn same_package(declared: &str, candidate: &str) -> bool {
    let fold = |name: &str| name.to_ascii_lowercase().replace('-', "_");
    fold(declared) == fold(candidate)
}

/// The component name for a source file: include-path root stripped, extension
/// dropped.
fn component_name(path: &Utf8Path) -> String {
    let mut parts: Vec<&str> = path.as_str().split('/').collect();

    // Strip the first source-root segment wherever it appears in the prefix, so
    // both `include/shop/order.hpp` and `libs/core/include/shop/order.hpp` reduce
    // to a name an include can name.
    if let Some(at) = parts.iter().position(|part| SOURCE_ROOTS.contains(part)) {
        parts.drain(..=at);
    }

    let joined = parts.join("/");
    strip_extension(&joined).to_owned()
}

/// A quoted include resolved against the directory of the file containing it.
fn beside_the_including_file(
    from: &Utf8Path,
    raw: &str,
    ctx: &ProjectContext<'_>,
) -> Option<String> {
    let base = from.parent()?;
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

    let candidate = component_name(Utf8Path::new(&parts.join("/")));
    ctx.known_modules.contains(&candidate).then_some(candidate)
}

// --- manifests ---------------------------------------------------------------

/// Parses `conanfile.txt`: an INI-like file whose sections say what each list is.
fn parse_conanfile_txt(path: &Utf8Path, text: &str) -> Manifest {
    let mut deps = Vec::new();
    let mut kind = None;

    for (index, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            kind = match section {
                "requires" => Some(DepKind::Runtime),
                // A build tool — CMake, a code generator — is never included from
                // source, so expecting an include would report every one.
                "tool_requires" | "build_requires" => Some(DepKind::Tooling),
                "test_requires" => Some(DepKind::Dev),
                _ => None,
            };
            continue;
        }

        let Some(kind) = kind else {
            continue;
        };
        if let Some(dep) = parse_reference(line, kind, path, index as u32 + 1) {
            deps.push(dep);
        }
    }

    finish(deps, None)
}

/// `fmt/10.2.1@user/channel` → name `fmt`, version `10.2.1`.
fn parse_reference(line: &str, kind: DepKind, path: &Utf8Path, at: u32) -> Option<DeclaredDep> {
    let reference = line.split('@').next().unwrap_or(line).trim();
    let (name, version) = match reference.split_once('/') {
        Some((name, version)) => (name, version),
        None => (reference, ""),
    };
    let name = name.trim();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    Some(DeclaredDep {
        name: name.to_owned(),
        requirement: version.trim().to_owned(),
        kind,
        declared_at: Provenance::new(path, Some(at)),
    })
}

/// Parses `vcpkg.json`.
fn parse_vcpkg(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    let root: serde_json::Value = serde_json::from_str(text)?;
    let mut deps = Vec::new();

    if let Some(list) = root.get("dependencies").and_then(|v| v.as_array()) {
        for entry in list {
            // Either `"fmt"` or `{ "name": "fmt", "features": [...] }`.
            let name = entry
                .as_str()
                .map(str::to_owned)
                .or_else(|| {
                    entry
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
                .filter(|name| !name.is_empty());
            let Some(name) = name else { continue };

            // A host dependency is a build-time tool for the build machine.
            let kind = if entry
                .get("host")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                DepKind::Tooling
            } else {
                DepKind::Runtime
            };

            deps.push(DeclaredDep {
                requirement: entry
                    .get("version>=")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                declared_at: Provenance::new(path, find_line(text, &name)),
                name,
                kind,
            });
        }
    }

    let package_name = root.get("name").and_then(|v| v.as_str()).map(str::to_owned);
    Ok(finish(deps, package_name))
}

/// Parses `conanfile.py`, which is a Python class definition.
///
/// Read with the Python grammar, taking the two declarative shapes: a
/// `requires = [...]` class attribute, and `self.requires("fmt/10.2.1")` inside
/// `requirements()`. A reference built from a variable contributes nothing rather
/// than a guess — the same rule as `Gemfile` and `Package.swift`.
fn parse_conanfile_py(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Python grammar failed: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let source = text.as_bytes();
    let mut deps = Vec::new();
    let mut package_name = None;
    collect_conan_python(tree.root_node(), source, path, &mut deps, &mut package_name);

    Ok(finish(deps, package_name))
}

fn collect_conan_python(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    path: &Utf8Path,
    deps: &mut Vec<DeclaredDep>,
    package_name: &mut Option<String>,
) {
    let line = node.start_position().row as u32 + 1;

    match node.kind() {
        "assignment" => {
            let target = node
                .child_by_field_name("left")
                .and_then(|left| left.utf8_text(source).ok())
                .unwrap_or_default();
            let kind = match target {
                "requires" => Some(DepKind::Runtime),
                "tool_requires" | "build_requires" => Some(DepKind::Tooling),
                "test_requires" => Some(DepKind::Dev),
                "name" => {
                    *package_name = node
                        .child_by_field_name("right")
                        .and_then(|right| python_string(right, source));
                    None
                }
                _ => None,
            };

            if let (Some(kind), Some(right)) = (kind, node.child_by_field_name("right")) {
                let mut cursor = right.walk();
                for item in right.named_children(&mut cursor) {
                    if let Some(reference) = python_string(item, source)
                        && let Some(dep) = parse_reference(
                            &reference,
                            kind,
                            path,
                            item.start_position().row as u32 + 1,
                        )
                    {
                        deps.push(dep);
                    }
                }
                // A single string rather than a list: `requires = "fmt/10.2.1"`.
                if let Some(reference) = python_string(right, source)
                    && let Some(dep) = parse_reference(&reference, kind, path, line)
                {
                    deps.push(dep);
                }
            }
        }
        "call" => {
            let function = node
                .child_by_field_name("function")
                .and_then(|function| function.utf8_text(source).ok())
                .unwrap_or_default();
            let kind = match function {
                "self.requires" => Some(DepKind::Runtime),
                "self.tool_requires" | "self.build_requires" => Some(DepKind::Tooling),
                "self.test_requires" => Some(DepKind::Dev),
                _ => None,
            };
            if let Some(kind) = kind
                && let Some(arguments) = node.child_by_field_name("arguments")
            {
                let mut cursor = arguments.walk();
                for argument in arguments.named_children(&mut cursor) {
                    if let Some(reference) = python_string(argument, source)
                        && let Some(dep) = parse_reference(&reference, kind, path, line)
                    {
                        deps.push(dep);
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_conan_python(child, source, path, deps, package_name);
    }
}

/// The literal content of a Python string node, or `None` when it is an f-string
/// or built from an expression.
fn python_string(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let mut cursor = node.walk();
    let children: Vec<tree_sitter::Node<'_>> = node.named_children(&mut cursor).collect();
    if children.iter().any(|child| child.kind() == "interpolation") {
        return None;
    }
    children
        .iter()
        .find(|child| child.kind() == "string_content")
        .and_then(|content| content.utf8_text(source).ok())
        .map(str::to_owned)
}

fn finish(mut deps: Vec<DeclaredDep>, package_name: Option<String>) -> Manifest {
    deps.sort_by(|a, b| (&a.name, a.kind).cmp(&(&b.name, b.kind)));
    deps.dedup_by(|a, b| a.name == b.name);
    Manifest { deps, package_name }
}

fn find_line(text: &str, needle: &str) -> Option<u32> {
    text.lines()
        .position(|line| line.contains(needle))
        .map(|index| index as u32 + 1)
}

// --- include extraction ------------------------------------------------------

/// Extracts every `#include` from one translation unit or header.
///
/// The two forms are kept apart by [`ImportForm`]: `"…"` searches the including
/// file's directory first and is a path, `<…>` goes straight to the include path
/// and usually names a package.
fn extract_includes(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_cpp::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the C++ grammar failed: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect_includes(tree.root_node(), text.as_bytes(), &mut imports);
    Ok(imports)
}

fn collect_includes(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    if node.kind() == "preproc_include" {
        if let Some(path_node) = node.child_by_field_name("path")
            && let Ok(raw) = path_node.utf8_text(source)
        {
            let line = node.start_position().row as u32 + 1;
            let trimmed = raw.trim_matches(['"', '<', '>']).to_owned();
            // `#include MACRO` is a computed include and names nothing readable.
            if !trimmed.is_empty() {
                out.push(if raw.starts_with('"') {
                    Import::path_reference(trimmed, line)
                } else {
                    Import::statement(trimmed, line)
                });
            }
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_includes(child, source, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;
    use tropism_core::model::Project;
    use tropism_core::provider::ImportForm;

    fn includes(source: &str) -> Vec<(String, ImportForm)> {
        extract_includes(Utf8Path::new("a.cpp"), source)
            .unwrap()
            .into_iter()
            .map(|import| (import.raw, import.form))
            .collect()
    }

    // --- conanfile.txt ----------------------------------------------------

    #[test]
    fn parses_conanfile_sections() {
        let manifest = parse_conanfile_txt(
            Utf8Path::new("conanfile.txt"),
            "[requires]\nfmt/10.2.1\nspdlog/1.13.0\n\n[tool_requires]\ncmake/3.29.0\n\n[test_requires]\ngtest/1.14.0\n\n[generators]\nCMakeDeps\n",
        );
        let named = |name: &str| manifest.deps.iter().find(|d| d.name == name).unwrap();
        assert_eq!(named("fmt").requirement, "10.2.1");
        assert_eq!(named("fmt").kind, DepKind::Runtime);
        assert_eq!(
            named("cmake").kind,
            DepKind::Tooling,
            "a build tool is never included from source"
        );
        assert_eq!(named("gtest").kind, DepKind::Dev);
        assert!(
            manifest.deps.iter().all(|d| d.name != "CMakeDeps"),
            "[generators] is not a dependency list"
        );
    }

    #[test]
    fn a_user_channel_suffix_is_not_part_of_the_name() {
        let dep = parse_reference(
            "fmt/10.2.1@company/stable",
            DepKind::Runtime,
            Utf8Path::new("conanfile.txt"),
            1,
        )
        .unwrap();
        assert_eq!(dep.name, "fmt");
        assert_eq!(dep.requirement, "10.2.1");
    }

    // --- vcpkg.json -------------------------------------------------------

    #[test]
    fn parses_vcpkg_string_and_object_dependencies() {
        let manifest = parse_vcpkg(
            Utf8Path::new("vcpkg.json"),
            r#"{"name":"shop","dependencies":["fmt",{"name":"boost","features":["system"]},{"name":"cmake","host":true}]}"#,
        )
        .unwrap();
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["boost", "cmake", "fmt"]);
        assert_eq!(manifest.package_name.as_deref(), Some("shop"));
        assert_eq!(
            manifest
                .deps
                .iter()
                .find(|d| d.name == "cmake")
                .unwrap()
                .kind,
            DepKind::Tooling,
            "a host dependency builds, it is not included"
        );
    }

    // --- conanfile.py -----------------------------------------------------

    #[test]
    fn parses_a_conanfile_class_attribute() {
        let manifest = parse_conanfile_py(
            Utf8Path::new("conanfile.py"),
            "from conan import ConanFile\n\nclass Shop(ConanFile):\n    name = \"shop\"\n    requires = [\"fmt/10.2.1\", \"spdlog/1.13.0\"]\n    tool_requires = \"cmake/3.29.0\"\n",
        )
        .unwrap();
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["cmake", "fmt", "spdlog"]);
        assert_eq!(manifest.package_name.as_deref(), Some("shop"));
        assert_eq!(
            manifest
                .deps
                .iter()
                .find(|d| d.name == "cmake")
                .unwrap()
                .kind,
            DepKind::Tooling
        );
    }

    #[test]
    fn parses_requirements_declared_in_a_method() {
        let manifest = parse_conanfile_py(
            Utf8Path::new("conanfile.py"),
            "class Shop(ConanFile):\n    def requirements(self):\n        self.requires(\"fmt/10.2.1\")\n        self.test_requires(\"gtest/1.14.0\")\n",
        )
        .unwrap();
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["fmt", "gtest"]);
    }

    /// The manifest is a program, and an f-string reference names a package that
    /// cannot be known without running it.
    #[test]
    fn an_interpolated_reference_is_skipped_rather_than_guessed() {
        let manifest = parse_conanfile_py(
            Utf8Path::new("conanfile.py"),
            "class S(ConanFile):\n    def requirements(self):\n        self.requires(f\"fmt/{self.version}\")\n        self.requires(\"spdlog/1.13.0\")\n",
        )
        .unwrap();
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["spdlog"]);
    }

    // --- include extraction -----------------------------------------------

    #[test]
    fn extracts_angle_and_quoted_includes_distinctly() {
        assert_eq!(
            includes("#include <vector>\n#include \"shop/order.hpp\"\n"),
            vec![
                ("vector".to_owned(), ImportForm::Statement),
                ("shop/order.hpp".to_owned(), ImportForm::PathReference),
            ]
        );
    }

    /// The reason for a grammar instead of a regex.
    #[test]
    fn ignores_includes_in_strings_and_comments() {
        let source = concat!(
            "#include <vector>\n",
            "// #include <fake_commented>\n",
            "/* #include <fake_block> */\n",
            "const char* s = \"#include <fake_quoted>\";\n",
        );
        assert_eq!(
            includes(source)
                .into_iter()
                .map(|(raw, _)| raw)
                .collect::<Vec<_>>(),
            vec!["vector"]
        );
    }

    #[test]
    fn reports_accurate_line_numbers() {
        let imports = extract_includes(
            Utf8Path::new("a.cpp"),
            "#include <vector>\n\n#include <string>\n",
        )
        .unwrap();
        assert_eq!(imports[0].line, 1);
        assert_eq!(imports[1].line, 3);
    }

    // --- module identity --------------------------------------------------

    fn component(path: &str) -> String {
        CppProvider
            .module_id_for_file(Utf8Path::new(path), "", "ignored")
            .name
    }

    /// A header and its translation unit are one module. Two would make every
    /// component appear in the graph twice, and a `.cpp` including its own `.hpp`
    /// an edge between them.
    #[test]
    fn a_header_and_its_source_are_the_same_component() {
        assert_eq!(component("include/shop/order.hpp"), "shop/order");
        assert_eq!(component("src/shop/order.cpp"), "shop/order");
    }

    #[test]
    fn a_nested_source_root_is_stripped_too() {
        assert_eq!(
            component("libs/core/include/shop/order.hpp"),
            "shop/order",
            "the include path root is what an #include never names"
        );
    }

    #[test]
    fn test_sources_are_external_test_modules() {
        assert!(
            CppProvider
                .module_id_for_file(Utf8Path::new("tests/order_test.cpp"), "", "tests")
                .kind
                .is_test()
        );
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
            language: Language::Cpp,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: String::new(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("conanfile.txt", Some(1)),
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
        CppProvider.resolve_import(&import, Utf8Path::new(file), &ctx)
    }

    /// A standard header has no directory and no extension. A structural rule
    /// rather than a list, so it stays correct as the standard library grows.
    #[test]
    fn resolves_standard_headers_structurally() {
        for header in ["vector", "string_view", "cstdio", "expected"] {
            assert_eq!(
                resolve("app/a.cpp", header, ImportForm::Statement, &[], &[]),
                ImportTarget::Stdlib,
                "{header}"
            );
        }
    }

    #[test]
    fn resolves_c_and_posix_headers() {
        for header in ["stdio.h", "unistd.h", "sys/stat.h"] {
            assert_eq!(
                resolve("app/a.cpp", header, ImportForm::Statement, &[], &[]),
                ImportTarget::Stdlib,
                "{header}"
            );
        }
    }

    #[test]
    fn resolves_an_include_path_header_to_its_component() {
        assert_eq!(
            resolve(
                "app/src/main.cpp",
                "shop/order.hpp",
                ImportForm::PathReference,
                &[],
                &["shop/order"]
            ),
            ImportTarget::Internal("shop/order".to_owned())
        );
    }

    #[test]
    fn resolves_a_declared_package_by_its_first_segment() {
        assert_eq!(
            resolve(
                "app/a.cpp",
                "fmt/format.h",
                ImportForm::Statement,
                &["fmt"],
                &[]
            ),
            ImportTarget::External("fmt".to_owned())
        );
    }

    /// A header at the root of the include path names its package in the stem.
    #[test]
    fn a_bare_third_party_header_names_its_package() {
        assert_eq!(
            resolve(
                "app/a.cpp",
                "sqlite3.h",
                ImportForm::Statement,
                &["sqlite3"],
                &[]
            ),
            ImportTarget::External("sqlite3".to_owned())
        );
    }

    /// Conan and vcpkg disagree about `-` and `_`; both spell the same package.
    #[test]
    fn package_names_match_across_separator_conventions() {
        assert_eq!(
            resolve(
                "app/a.cpp",
                "yaml-cpp/yaml.h",
                ImportForm::Statement,
                &["yaml_cpp"],
                &[]
            ),
            ImportTarget::External("yaml_cpp".to_owned())
        );
    }

    #[test]
    fn resolves_the_curated_residue() {
        assert_eq!(
            resolve(
                "app/a.cpp",
                "nlohmann/json.hpp",
                ImportForm::Statement,
                &[],
                &[]
            ),
            ImportTarget::External("nlohmann_json".to_owned())
        );
    }

    #[test]
    fn an_undeclared_package_is_external_so_it_is_reported_missing() {
        assert_eq!(
            resolve(
                "app/a.cpp",
                "spdlog/spdlog.h",
                ImportForm::Statement,
                &[],
                &[]
            ),
            ImportTarget::External("spdlog".to_owned())
        );
    }

    // --- lockfile ---------------------------------------------------------

    #[test]
    fn no_cpp_lockfile_is_treated_as_a_resolved_tree() {
        let parsed = CppProvider
            .parse_lockfile(
                Utf8Path::new("conan.lock"),
                "{\"requires\":[\"fmt/10.2.1\"]}",
            )
            .unwrap();
        assert!(parsed.is_none());
        assert!(
            CppProvider
                .resolved_tree_note()
                .unwrap()
                .contains("conan.lock")
        );
    }
}
