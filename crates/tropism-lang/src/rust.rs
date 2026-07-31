//! Rust provider.
//!
//! Implemented so tropism can be run against itself. Rust is the third ecosystem and
//! the first where the module path in an import is *not* a file path: `use
//! crate::report::Finding` names a module and then an item inside it, with no
//! syntax marking the boundary. Resolution therefore matches the longest prefix
//! that corresponds to a real module file.

use std::collections::BTreeSet;

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{
    Import, ImportForm, ImportTarget, LanguageProvider, ProjectContext, VersionOps,
};

pub struct RustProvider;

/// Roots that need no declaration. `test` is the unstable internal crate, not the
/// `test` attribute.
const BUILTIN_ROOTS: &[&str] = &["std", "core", "alloc", "proc_macro", "test"];

/// Directories whose files are compiled as separate units that may freely use the
/// crate under test, so they can never form a cycle with it.
const SEPARATE_TARGET_DIRS: &[&str] = &["tests", "benches", "examples"];

struct CargoVersionOps;

impl VersionOps for CargoVersionOps {
    /// Cargo versions are SemVer and `semver` would answer this correctly, but no
    /// current check needs an ordering — duplicate detection compares for equality.
    /// `None` is honest; a lexical comparison would be wrong.
    fn compare(&self, _a: &str, _b: &str) -> Option<std::cmp::Ordering> {
        None
    }

    fn satisfies(&self, _version: &str, _requirement: &str) -> Option<bool> {
        None
    }
}

impl LanguageProvider for RustProvider {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["Cargo.toml"]
    }

    fn lockfile_names(&self) -> &'static [&'static str] {
        &["Cargo.lock"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        parse_cargo_toml(path, text)
    }

    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        parse_cargo_lock(text)
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_rust_imports(path, text)
    }

    /// A Rust module is a file, but the mapping is not the path: `src/lib.rs` is the
    /// crate root, `src/a/mod.rs` is module `a`, and `src/a/b.rs` is `a::b`.
    fn module_id_for_file(&self, path: &Utf8Path, _text: &str, _default_id: &str) -> ModuleId {
        let (name, separate_target) = module_path_of(path);
        if separate_target {
            // An integration test or benchmark is its own crate and uses the library
            // through its public interface, so it can never be part of a cycle
            // with it.
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
        let path = import.raw.trim_start_matches("::");
        let mut segments = path.split("::").filter(|s| !s.is_empty());
        let Some(root) = segments.next() else {
            return ImportTarget::Unresolved {
                reason: "empty use path".to_owned(),
            };
        };
        let rest: Vec<&str> = segments.collect();

        let current = module_path_of(from).0;

        match root {
            "crate" => contain(resolve_internal(&rest, ctx), &current),
            "self" => {
                let (current, _) = module_path_of(from);
                let mut full = split_module(&current);
                full.extend(rest.iter().map(|s| (*s).to_owned()));
                contain(resolve_internal_owned(&full, ctx), &current)
            }
            "super" => {
                let (current, _) = module_path_of(from);
                let mut full = split_module(&current);
                full.pop();
                full.extend(rest.iter().map(|s| (*s).to_owned()));
                contain(resolve_internal_owned(&full, ctx), &current)
            }
            _ if BUILTIN_ROOTS.contains(&root) => ImportTarget::Stdlib,
            _ => {
                // Rust 2018 uniform paths: `pub use model::Thing` at the crate root
                // names a *local* module, with no `crate::` prefix to say so. Missing
                // this reported tropism-core's own modules as undeclared dependencies.
                let modules = module_set(ctx);
                if modules.contains(root) {
                    let mut full = vec![root.to_owned()];
                    full.extend(rest.iter().map(|s| (*s).to_owned()));
                    return contain(resolve_internal_owned(&full, ctx), &current);
                }

                // Cargo lets a dependency be named `tree-sitter-go` and imported as
                // `tree_sitter_go`, so matching is on the normalized form while the
                // *declared* spelling is what gets reported.
                let normalized = normalize_crate_name(root);
                let declared = ctx
                    .declared
                    .iter()
                    .map(|dep| dep.name.as_str())
                    .chain(ctx.sibling_packages.iter().map(String::as_str))
                    .find(|name| normalize_crate_name(name) == normalized);

                match (declared, import.form) {
                    (Some(name), _) => ImportTarget::External(name.to_owned()),
                    (None, ImportForm::Statement) => ImportTarget::External(root.to_owned()),
                    // A bare path naming nothing declared is far more likely to be a
                    // local type (`Palette::plain()`) than an undeclared crate. It
                    // must never invent a missing dependency.
                    (None, ImportForm::PathReference) => ImportTarget::Unresolved {
                        reason: format!("`{root}` is a path reference, not an import"),
                    },
                }
            }
        }
    }

    fn is_stdlib(&self, module: &str) -> bool {
        let root = module.split("::").next().unwrap_or(module);
        BUILTIN_ROOTS.contains(&root)
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &CargoVersionOps
    }
}

/// Whether `ancestor` contains `descendant` in the module tree.
fn is_ancestor(ancestor: &str, descendant: &str) -> bool {
    ancestor == "." || descendant == ancestor || descendant.starts_with(&format!("{ancestor}::"))
}

/// Rewrites a reference to one of the importing module's own ancestors as a
/// reference to itself, so it produces no edge.
///
/// In Rust a submodule is *part of* its parent, not a dependent of it: every crate
/// has `lib.rs` re-exporting its submodules and submodules reaching back with
/// `use super::*` or `use crate::…`. Modelling containment as dependency reported a
/// cycle in tropism-core — and would in essentially every Rust crate ever written.
/// Sibling and descendant references stay real edges.
fn contain(target: ImportTarget, current: &str) -> ImportTarget {
    match &target {
        ImportTarget::Internal(module) if is_ancestor(module, current) => {
            ImportTarget::Internal(current.to_owned())
        }
        _ => target,
    }
}

fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn split_module(module: &str) -> Vec<String> {
    if module == "." {
        Vec::new()
    } else {
        module.split("::").map(str::to_owned).collect()
    }
}

fn join_module(segments: &[String]) -> String {
    if segments.is_empty() {
        ".".to_owned()
    } else {
        segments.join("::")
    }
}

/// The module a source file defines, and whether it is a separate compilation
/// target.
///
/// `src/lib.rs` and `src/main.rs` are the crate root; `src/a/mod.rs` is `a`;
/// `src/a/b.rs` is `a::b`.
fn module_path_of(file: &Utf8Path) -> (String, bool) {
    let raw = file.as_str();

    let separate = SEPARATE_TARGET_DIRS
        .iter()
        .any(|dir| raw.starts_with(&format!("{dir}/")) || raw.contains(&format!("/{dir}/")));

    // Everything before `src/` (or the target dir) is the crate's own location.
    let tail = SEPARATE_TARGET_DIRS
        .iter()
        .chain(["src"].iter())
        .find_map(|dir| {
            let marker = format!("/{dir}/");
            raw.rsplit_once(&marker)
                .map(|(_, tail)| tail)
                .or_else(|| raw.strip_prefix(&format!("{dir}/")))
        })
        .unwrap_or(raw);

    let stem = tail.strip_suffix(".rs").unwrap_or(tail);
    let mut segments: Vec<&str> = stem.split('/').filter(|s| !s.is_empty()).collect();

    match segments.last() {
        Some(&"lib") | Some(&"main") | Some(&"mod") => {
            segments.pop();
        }
        _ => {}
    }

    let name = if segments.is_empty() {
        ".".to_owned()
    } else {
        segments.join("::")
    };
    (name, separate)
}

/// Every module the project defines, for longest-prefix matching.
fn module_set(ctx: &ProjectContext<'_>) -> BTreeSet<String> {
    ctx.source_files
        .iter()
        .filter_map(|file| {
            let relative = file.strip_prefix(&ctx.project.root).unwrap_or(file);
            let (name, separate) = module_path_of(relative);
            (!separate).then_some(name)
        })
        .collect()
}

fn resolve_internal(segments: &[&str], ctx: &ProjectContext<'_>) -> ImportTarget {
    let owned: Vec<String> = segments.iter().map(|s| (*s).to_owned()).collect();
    resolve_internal_owned(&owned, ctx)
}

/// Matches the longest prefix that is a real module.
///
/// `use crate::report::Finding` has no syntax separating the module path from the
/// item, so `report::Finding` must be tried before `report`. Falling back to the
/// crate root is correct: an item imported from `crate::Foo` lives in `lib.rs`.
fn resolve_internal_owned(segments: &[String], ctx: &ProjectContext<'_>) -> ImportTarget {
    let modules = module_set(ctx);
    for length in (0..=segments.len()).rev() {
        let candidate = join_module(&segments[..length]);
        if candidate == "." || modules.contains(&candidate) {
            return ImportTarget::Internal(candidate);
        }
    }
    ImportTarget::Internal(".".to_owned())
}

fn parse_cargo_toml(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    let root: toml::Value = toml::from_str(text)?;
    let mut manifest = Manifest {
        package_name: root
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(toml::Value::as_str)
            .map(str::to_owned),
        deps: Vec::new(),
    };

    collect_dependency_tables(&root, path, text, &mut manifest.deps);

    // `[target.'cfg(...)'.dependencies]` are real dependencies of some build
    // configuration. Extracting them regardless of the cfg is deliberate: tropism does
    // not evaluate conditions (design/03-language-providers.md).
    if let Some(targets) = root.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            collect_dependency_tables(target, path, text, &mut manifest.deps);
        }
    }

    // `[workspace.dependencies]` is a version catalogue, not a set of dependencies
    // anything actually uses. Treating it as declared would report every entry as
    // unused, since a virtual manifest has no source of its own.
    manifest
        .deps
        .sort_by(|a, b| (&a.name, a.kind).cmp(&(&b.name, b.kind)));
    manifest
        .deps
        .dedup_by(|a, b| a.name == b.name && a.kind == b.kind);
    Ok(manifest)
}

fn collect_dependency_tables(
    table: &toml::Value,
    path: &Utf8Path,
    text: &str,
    out: &mut Vec<DeclaredDep>,
) {
    for (field, kind) in [
        ("dependencies", DepKind::Runtime),
        ("dev-dependencies", DepKind::Dev),
        ("build-dependencies", DepKind::Build),
    ] {
        let Some(entries) = table.get(field).and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, spec) in entries {
            // `foo = { package = "bar" }` renames: `foo` is what code imports, so
            // `foo` is the name a usage check must match.
            let optional = spec
                .as_table()
                .and_then(|t| t.get("optional"))
                .and_then(toml::Value::as_bool)
                .unwrap_or(false);

            out.push(DeclaredDep {
                name: name.clone(),
                requirement: requirement_of(spec),
                kind: if optional { DepKind::Optional } else { kind },
                declared_at: Provenance::new(path, find_key_line(text, name)),
            });
        }
    }
}

fn requirement_of(spec: &toml::Value) -> String {
    match spec {
        toml::Value::String(version) => version.clone(),
        toml::Value::Table(table) => table
            .get("version")
            .and_then(toml::Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                table
                    .get("workspace")
                    .and_then(toml::Value::as_bool)
                    .and_then(|inherited| inherited.then(|| "workspace".to_owned()))
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// Best-effort line for a dependency key, so findings can cite one. `toml` discards
/// spans; a visibly wrong line beats a silently missing one.
fn find_key_line(text: &str, key: &str) -> Option<u32> {
    text.lines()
        .position(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with(['=', '.']))
                || trimmed.starts_with(&format!("\"{key}\""))
        })
        .map(|index| index as u32 + 1)
}

/// Parses `Cargo.lock` into a resolved graph.
///
/// Genuinely resolved, unlike `go.sum`: every package appears once per selected
/// version with its edges, so version-conflict and diamond questions are answerable
/// offline.
fn parse_cargo_lock(text: &str) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
    let root: toml::Value = toml::from_str(text)?;
    let Some(packages) = root.get("package").and_then(toml::Value::as_array) else {
        return Ok(None);
    };

    let entries: Vec<(String, String)> = packages
        .iter()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_owned();
            let version = entry.get("version")?.as_str()?.to_owned();
            Some((name, version))
        })
        .collect();

    let resolve = |spec: &str| -> Option<String> {
        // A dependency entry is `name` when unambiguous, or `name version` when the
        // lockfile carries more than one copy.
        let (name, version) = match spec.split_once(' ') {
            Some((name, version)) => (name, Some(version)),
            None => (spec, None),
        };
        entries
            .iter()
            .find(|(candidate, candidate_version)| {
                candidate == name && version.is_none_or(|wanted| candidate_version == wanted)
            })
            .map(|(name, version)| format!("{name} {version}"))
    };

    let mut resolved = Vec::new();
    for entry in packages {
        let (Some(name), Some(version)) = (
            entry.get("name").and_then(toml::Value::as_str),
            entry.get("version").and_then(toml::Value::as_str),
        ) else {
            continue;
        };

        let mut dependencies: Vec<String> = entry
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .map(|deps| {
                deps.iter()
                    .filter_map(toml::Value::as_str)
                    .filter_map(resolve)
                    .collect()
            })
            .unwrap_or_default();
        dependencies.sort();
        dependencies.dedup();

        resolved.push(ResolvedDep {
            key: format!("{name} {version}"),
            name: name.to_owned(),
            version: version.to_owned(),
            dependencies,
        });
    }

    resolved.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(Some(resolved))
}

fn extract_rust_imports(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Rust grammar failed: {error}"))?;

    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect(tree.root_node(), text.as_bytes(), &mut imports);
    imports.sort_by(|a, b| (a.line, &a.raw).cmp(&(b.line, &b.raw)));
    imports.dedup_by(|a, b| a.line == b.line && a.raw == b.raw);
    Ok(imports)
}

/// The leftmost segment of a scoped path: `a::b::c` yields `a`.
fn leftmost_root<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut current = node;
    loop {
        match current.kind() {
            "scoped_identifier" | "scoped_type_identifier" | "generic_type" => {
                current = current
                    .child_by_field_name("path")
                    .or_else(|| current.child_by_field_name("type"))?;
            }
            "identifier" | "crate" | "self" | "super" | "type_identifier" => return Some(current),
            _ => return None,
        }
    }
}

fn collect(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    // A fully-qualified path is a real use of a crate — `anyhow::Result<T>`,
    // `blake3::Hasher::new()`, `serde_json::json!{}` — and idiomatic Rust writes
    // many of them with no `use` statement anywhere. Extracting only `use` reported
    // most of this workspace's own dependencies as unused.
    if matches!(node.kind(), "scoped_identifier" | "scoped_type_identifier")
        && let Some(root) = leftmost_root(node)
        && let Ok(text) = root.utf8_text(source)
        && !matches!(text, "crate" | "self" | "super" | "Self")
    {
        out.push(Import::path_reference(
            text,
            root.start_position().row as u32 + 1,
        ));
    }

    // Macro arguments and attribute bodies are flat token trees, so a path inside
    // them never becomes a `scoped_identifier`. `#[derive(thiserror::Error)]` and
    // `eprintln!("{}", tropism_core::report::S)` are both real uses that the structured
    // walk above cannot see.
    if node.kind() == "token_tree" {
        let mut cursor = node.walk();
        let tokens: Vec<tree_sitter::Node<'_>> = node.children(&mut cursor).collect();
        for (index, token) in tokens.iter().enumerate() {
            if token.kind() == "identifier"
                && tokens
                    .get(index + 1)
                    .is_some_and(|next| next.kind() == "::")
                && let Ok(text) = token.utf8_text(source)
                && !matches!(text, "crate" | "self" | "super" | "Self")
            {
                out.push(Import::path_reference(
                    text,
                    token.start_position().row as u32 + 1,
                ));
            }
        }
    }

    match node.kind() {
        "use_declaration" => {
            if let Some(argument) = node.child_by_field_name("argument")
                && let Ok(raw) = argument.utf8_text(source)
                && let Some(prefix) = use_prefix(raw)
            {
                out.push(Import::statement(
                    prefix,
                    node.start_position().row as u32 + 1,
                ));
            }
            return;
        }
        "extern_crate_declaration" => {
            let mut cursor = node.walk();
            if let Some(name) = node
                .children(&mut cursor)
                .find(|child| child.kind() == "identifier")
                .and_then(|child| child.utf8_text(source).ok())
            {
                out.push(Import::statement(
                    name,
                    node.start_position().row as u32 + 1,
                ));
            }
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, out);
    }
}

/// The module prefix of a `use` path.
///
/// `std::{fmt, io}` yields `std`; `crate::render::text::Palette` yields itself. The
/// braces are dropped because the graph only needs the prefix, and the trailing
/// item name is disambiguated later by longest-prefix matching against real modules.
fn use_prefix(raw: &str) -> Option<String> {
    let cleaned: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let head = cleaned.split('{').next()?.trim();
    let head = head.split(" as ").next()?.trim();
    let head = head.trim_end_matches("::*").trim_end_matches("::").trim();
    (!head.is_empty()).then(|| head.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tropism_core::model::Project;

    fn extract(source: &str) -> Vec<String> {
        extract_rust_imports(Utf8Path::new("src/lib.rs"), source)
            .unwrap()
            .into_iter()
            .map(|i| i.raw)
            .collect()
    }

    // --- Cargo.toml -------------------------------------------------------

    #[test]
    fn parses_the_package_name_and_dependency_kinds() {
        let manifest = parse_cargo_toml(
            Utf8Path::new("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[dependencies]\nserde = \"1\"\n\n\
             [dev-dependencies]\ninsta = \"1\"\n\n[build-dependencies]\ncc = \"1\"\n",
        )
        .unwrap();

        assert_eq!(manifest.package_name.as_deref(), Some("app"));
        let kinds: Vec<(&str, DepKind)> = manifest
            .deps
            .iter()
            .map(|d| (d.name.as_str(), d.kind))
            .collect();
        assert!(kinds.contains(&("serde", DepKind::Runtime)));
        assert!(kinds.contains(&("insta", DepKind::Dev)));
        assert!(kinds.contains(&("cc", DepKind::Build)));
    }

    #[test]
    fn records_a_workspace_inherited_requirement() {
        let manifest = parse_cargo_toml(
            Utf8Path::new("Cargo.toml"),
            "[package]\nname = \"a\"\n[dependencies]\nserde.workspace = true\n",
        )
        .unwrap();
        assert_eq!(manifest.deps[0].name, "serde");
        assert_eq!(manifest.deps[0].requirement, "workspace");
    }

    #[test]
    fn treats_an_optional_dependency_as_optional() {
        let manifest = parse_cargo_toml(
            Utf8Path::new("Cargo.toml"),
            "[package]\nname = \"a\"\n[dependencies]\nratatui = { version = \"1\", optional = true }\n",
        )
        .unwrap();
        assert_eq!(manifest.deps[0].kind, DepKind::Optional);
    }

    #[test]
    fn collects_platform_specific_dependencies() {
        let manifest = parse_cargo_toml(
            Utf8Path::new("Cargo.toml"),
            "[package]\nname = \"a\"\n[target.'cfg(unix)'.dependencies]\nnix = \"0.29\"\n",
        )
        .unwrap();
        assert_eq!(manifest.deps[0].name, "nix");
    }

    /// A version catalogue, not a set of dependencies anything uses. Treating it as
    /// declared would report every entry unused, since a virtual manifest has no
    /// source of its own.
    #[test]
    fn workspace_dependencies_are_not_declared_dependencies() {
        let manifest = parse_cargo_toml(
            Utf8Path::new("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        assert!(manifest.deps.is_empty(), "got {:?}", manifest.deps);
        assert_eq!(
            manifest.package_name, None,
            "a virtual manifest has no package"
        );
    }

    // --- Cargo.lock -------------------------------------------------------

    const LOCK: &str = r#"
version = 4

[[package]]
name = "app"
version = "0.1.0"
dependencies = ["shared 2.0.0", "helper"]

[[package]]
name = "helper"
version = "1.0.0"
dependencies = ["shared 1.0.0"]

[[package]]
name = "shared"
version = "1.0.0"

[[package]]
name = "shared"
version = "2.0.0"
"#;

    #[test]
    fn builds_a_resolved_tree_from_the_lockfile() {
        let resolved = parse_cargo_lock(LOCK).unwrap().unwrap();
        assert_eq!(resolved.len(), 4);
        assert!(resolved.iter().any(|d| d.key == "shared 1.0.0"));
        assert!(resolved.iter().any(|d| d.key == "shared 2.0.0"));
    }

    /// A dependency entry carries a version only when the name is ambiguous, so both
    /// forms have to resolve.
    #[test]
    fn resolves_versioned_and_bare_dependency_entries() {
        let resolved = parse_cargo_lock(LOCK).unwrap().unwrap();
        let app = resolved.iter().find(|d| d.name == "app").unwrap();
        assert!(app.dependencies.contains(&"shared 2.0.0".to_owned()));
        assert!(app.dependencies.contains(&"helper 1.0.0".to_owned()));
    }

    #[test]
    fn a_lockfile_without_packages_yields_no_tree() {
        assert!(parse_cargo_lock("version = 4\n").unwrap().is_none());
    }

    // --- import extraction ------------------------------------------------

    #[test]
    fn extracts_a_simple_use() {
        assert_eq!(extract("use serde::Serialize;\n"), vec!["serde::Serialize"]);
    }

    #[test]
    fn reduces_a_braced_group_to_its_prefix() {
        assert_eq!(extract("use std::{fmt, io};\n"), vec!["std"]);
        assert_eq!(
            extract("use crate::render::{text, tui};\n"),
            vec!["crate::render"]
        );
    }

    #[test]
    fn handles_glob_and_alias_forms() {
        assert_eq!(extract("use std::io::*;\n"), vec!["std::io"]);
        assert_eq!(
            extract("use std::fmt::Write as _;\n"),
            vec!["std::fmt::Write"]
        );
    }

    #[test]
    fn handles_a_leading_colon_path() {
        assert_eq!(
            extract("use ::serde::Serialize;\n"),
            vec!["::serde::Serialize"]
        );
    }

    #[test]
    fn extracts_extern_crate() {
        assert_eq!(extract("extern crate alloc;\n"), vec!["alloc"]);
    }

    /// The reason for a grammar rather than a regex.
    #[test]
    fn ignores_use_like_text_in_strings_and_comments() {
        let source = concat!(
            "use serde::Serialize;\n",
            "// use fake::Commented;\n",
            "/* use fake::Blocked; */\n",
            "const S: &str = \"use fake::Quoted;\";\n",
        );
        assert_eq!(extract(source), vec!["serde::Serialize"]);
    }

    /// Conditional compilation is not evaluated: an import behind a disabled cfg is
    /// still a real dependency.
    #[test]
    fn extracts_imports_behind_a_cfg_attribute() {
        let source = "#[cfg(feature = \"tui\")]\nuse ratatui::Frame;\n";
        assert_eq!(extract(source), vec!["ratatui::Frame"]);
    }

    #[test]
    fn extracts_a_use_nested_inside_a_function() {
        assert_eq!(
            extract("fn f() {\n    use std::io::Read;\n}\n"),
            vec!["std::io::Read"]
        );
    }

    // --- module paths -----------------------------------------------------

    #[test]
    fn maps_files_to_rust_module_paths() {
        assert_eq!(module_path_of(Utf8Path::new("src/lib.rs")).0, ".");
        assert_eq!(module_path_of(Utf8Path::new("src/main.rs")).0, ".");
        assert_eq!(module_path_of(Utf8Path::new("src/report.rs")).0, "report");
        assert_eq!(
            module_path_of(Utf8Path::new("src/render/mod.rs")).0,
            "render"
        );
        assert_eq!(
            module_path_of(Utf8Path::new("src/render/text.rs")).0,
            "render::text"
        );
    }

    #[test]
    fn integration_tests_and_benches_are_separate_targets() {
        assert!(module_path_of(Utf8Path::new("tests/go_pipeline.rs")).1);
        assert!(module_path_of(Utf8Path::new("benches/bench.rs")).1);
        assert!(!module_path_of(Utf8Path::new("src/lib.rs")).1);
    }

    #[test]
    fn a_separate_target_becomes_an_external_test_module() {
        let id = RustProvider.module_id_for_file(Utf8Path::new("tests/x.rs"), "", "tests");
        assert_eq!(id, ModuleId::external_test("x"));
    }

    // --- resolution -------------------------------------------------------

    fn resolve(from: &str, path: &str, files: &[&str], declared: &[&str]) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::new(),
            language: Language::Rust,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: "1".to_owned(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("Cargo.toml", Some(1)),
            })
            .collect();
        let source_files: Vec<Utf8PathBuf> = files.iter().map(Utf8PathBuf::from).collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: Some("app"),
            declared: &deps,
            sibling_packages: &[],
            known_modules: &BTreeSet::new(),
            source_files: &source_files,
        };
        let import = Import::statement(path, 1);
        RustProvider.resolve_import(&import, Utf8Path::new(from), &ctx)
    }

    #[test]
    fn resolves_std_and_friends_as_stdlib() {
        for path in [
            "std::fmt",
            "core::mem",
            "alloc::vec",
            "proc_macro::TokenStream",
        ] {
            assert_eq!(
                resolve("src/lib.rs", path, &[], &[]),
                ImportTarget::Stdlib,
                "{path}"
            );
        }
    }

    /// `report` is a module and `Finding` is an item inside it, with no syntax
    /// marking the boundary — so the longest matching prefix wins.
    #[test]
    fn resolves_a_crate_path_to_the_longest_real_module() {
        assert_eq!(
            resolve(
                "src/lib.rs",
                "crate::report::Finding",
                &["src/report.rs"],
                &[]
            ),
            ImportTarget::Internal("report".to_owned())
        );
        assert_eq!(
            resolve(
                "src/lib.rs",
                "crate::render::text::Palette",
                &["src/render/mod.rs", "src/render/text.rs"],
                &[]
            ),
            ImportTarget::Internal("render::text".to_owned())
        );
    }

    #[test]
    fn an_item_at_the_crate_root_resolves_to_the_root_module() {
        assert_eq!(
            resolve("src/lib.rs", "crate::Finding", &["src/lib.rs"], &[]),
            ImportTarget::Internal(".".to_owned())
        );
    }

    #[test]
    fn resolves_super_relative_to_the_importing_file() {
        assert_eq!(
            resolve(
                "src/render/text.rs",
                "super::testdata",
                &[
                    "src/render/mod.rs",
                    "src/render/text.rs",
                    "src/render/testdata.rs"
                ],
                &[]
            ),
            ImportTarget::Internal("render::testdata".to_owned())
        );
    }

    #[test]
    fn resolves_self_relative_to_the_importing_file() {
        assert_eq!(
            resolve(
                "src/render/mod.rs",
                "self::text",
                &["src/render/mod.rs", "src/render/text.rs"],
                &[]
            ),
            ImportTarget::Internal("render::text".to_owned())
        );
    }

    /// Cargo names a dependency `tree-sitter-go`; code imports `tree_sitter_go`.
    #[test]
    fn matches_a_hyphenated_dependency_to_its_underscored_import() {
        assert_eq!(
            resolve(
                "src/lib.rs",
                "tree_sitter_go::LANGUAGE",
                &[],
                &["tree-sitter-go"]
            ),
            ImportTarget::External("tree-sitter-go".to_owned()),
            "the declared spelling is what gets reported"
        );
    }

    /// Rust 2018 uniform paths: `pub use model::Thing` at the crate root names a
    /// *local* module with no `crate::` prefix. Missing this reported tropism-core's own
    /// modules as undeclared dependencies.
    #[test]
    fn a_bare_root_naming_a_local_module_is_internal() {
        assert_eq!(
            resolve(
                "src/lib.rs",
                "model::Language",
                &["src/lib.rs", "src/model.rs"],
                &[]
            ),
            ImportTarget::Internal("model".to_owned())
        );
    }

    /// A submodule is part of its parent, not a dependent of it. Every Rust crate
    /// has `lib.rs` re-exporting submodules that reach back with `use super::*`;
    /// modelling that as a dependency reported a cycle in tropism-core itself.
    #[test]
    fn a_reference_to_an_ancestor_module_produces_no_edge() {
        // `graph`'s test module does `use super::*`, reaching the crate root.
        assert_eq!(
            resolve(
                "src/graph.rs",
                "super",
                &["src/lib.rs", "src/graph.rs"],
                &[]
            ),
            ImportTarget::Internal("graph".to_owned()),
            "resolves to itself, so add_edge drops it"
        );
        assert_eq!(
            resolve(
                "src/render/text.rs",
                "crate::render",
                &["src/lib.rs", "src/render/mod.rs", "src/render/text.rs"],
                &[]
            ),
            ImportTarget::Internal("render::text".to_owned())
        );
    }

    #[test]
    fn a_sibling_reference_is_still_a_real_edge() {
        assert_eq!(
            resolve(
                "src/analysis.rs",
                "crate::report::Finding",
                &["src/lib.rs", "src/analysis.rs", "src/report.rs"],
                &[]
            ),
            ImportTarget::Internal("report".to_owned())
        );
    }

    /// Idiomatic Rust writes `anyhow::Result<T>` with no `use` anywhere. Extracting
    /// only `use` statements reported most of this workspace's dependencies unused.
    #[test]
    fn a_fully_qualified_path_counts_as_usage() {
        let imports = extract_rust_imports(
            Utf8Path::new("src/lib.rs"),
            "fn f() -> anyhow::Result<()> { blake3::Hasher::new(); Ok(()) }\n",
        )
        .unwrap();
        let roots: Vec<&str> = imports.iter().map(|i| i.raw.as_str()).collect();
        assert!(roots.contains(&"anyhow"), "got {roots:?}");
        assert!(roots.contains(&"blake3"), "got {roots:?}");
        assert!(imports.iter().all(|i| i.form == ImportForm::PathReference));
    }

    /// Macro arguments and attribute bodies are flat token trees, so paths inside
    /// them never become scoped identifiers.
    #[test]
    fn paths_inside_macros_and_attributes_count_as_usage() {
        let imports = extract_rust_imports(
            Utf8Path::new("src/lib.rs"),
            "#[derive(Debug, thiserror::Error)]\nstruct E;\n\
             fn f() { eprintln!(\"{}\", tropism_core::report::S); }\n",
        )
        .unwrap();
        let roots: Vec<&str> = imports.iter().map(|i| i.raw.as_str()).collect();
        assert!(
            roots.contains(&"thiserror"),
            "derive attribute: got {roots:?}"
        );
        assert!(
            roots.contains(&"tropism_core"),
            "macro argument: got {roots:?}"
        );
    }

    /// `Palette::plain()` is a local type, not an undeclared crate. A path reference
    /// must never invent a missing dependency.
    #[test]
    fn an_unknown_path_reference_never_becomes_a_missing_dependency() {
        let project = Project {
            root: Utf8PathBuf::new(),
            language: Language::Rust,
            manifests: vec![],
            lockfile: None,
        };
        let ctx = ProjectContext {
            project: &project,
            package_name: Some("app"),
            declared: &[],
            sibling_packages: &[],
            known_modules: &BTreeSet::new(),
            source_files: &[],
        };
        let import = Import::path_reference("Palette", 1);
        assert!(matches!(
            RustProvider.resolve_import(&import, Utf8Path::new("src/lib.rs"), &ctx),
            ImportTarget::Unresolved { .. }
        ));
    }

    #[test]
    fn an_undeclared_crate_is_reported_under_its_imported_name() {
        assert_eq!(
            resolve("src/lib.rs", "mystery::Thing", &[], &[]),
            ImportTarget::External("mystery".to_owned())
        );
    }
}
