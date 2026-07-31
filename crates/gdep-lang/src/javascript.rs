//! JavaScript and TypeScript provider.
//!
//! Chosen as the second language deliberately: npm has no built-in `tidy`
//! equivalent, so the incumbent is a third-party tool rather than the package
//! manager itself, and `package-lock.json` is a genuinely resolved graph. That
//! makes it the first ecosystem where all six checks can run. See
//! `design/09-product-review.md`.

use std::collections::BTreeMap;

use camino::Utf8Path;
use gdep_core::graph::ModuleId;
use gdep_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use gdep_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};

pub struct JavaScriptProvider;

/// Node's built-in modules. The `node:` prefix is always a builtin regardless.
const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Extensions a relative specifier may resolve to, in Node/TypeScript order.
const RESOLUTION_EXTENSIONS: &[&str] =
    &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "d.ts"];

struct NpmVersionOps;

impl VersionOps for NpmVersionOps {
    /// Not implemented. npm versions are SemVer and could be compared, but nothing
    /// in the current checks needs an ordering — duplicate detection compares for
    /// equality only. Returning `None` beats a wrong lexical answer.
    fn compare(&self, _a: &str, _b: &str) -> Option<std::cmp::Ordering> {
        None
    }

    fn satisfies(&self, _version: &str, _requirement: &str) -> Option<bool> {
        None
    }
}

impl LanguageProvider for JavaScriptProvider {
    fn language(&self) -> Language {
        Language::JavaScript
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["package.json"]
    }

    /// npm only. yarn and pnpm lockfiles are unrelated formats; yarn's is bespoke
    /// and pnpm's is YAML, which has no maintained crate — see
    /// `design/08-crates.md`. A repo using those gets `Unavailable`, correctly.
    fn lockfile_names(&self) -> &'static [&'static str] {
        &["package-lock.json"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts"]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        parse_package_json(path, text)
    }

    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        parse_package_lock(text)
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_js_imports(path, text)
    }

    /// A JavaScript module is a *file*, not a directory.
    ///
    /// This is the opposite of Go and it matters: file-level cycles are the common
    /// and genuinely painful case in JS/TS, and a directory-level graph would miss
    /// every one of them.
    fn module_id_for_file(&self, path: &Utf8Path, _text: &str, _default_id: &str) -> ModuleId {
        ModuleId::module(strip_module_extension(path))
    }

    fn resolve_import(
        &self,
        import: &Import,
        from: &Utf8Path,
        ctx: &ProjectContext<'_>,
    ) -> ImportTarget {
        let specifier = import.raw.as_str();

        if specifier.is_empty() {
            return ImportTarget::Unresolved {
                reason: "empty specifier".to_owned(),
            };
        }

        // Node subpath imports declared in package.json's "imports" field.
        if specifier.starts_with('#') {
            return ImportTarget::Internal(specifier.to_owned());
        }

        if specifier.starts_with('.') || specifier.starts_with('/') {
            return resolve_relative(specifier, from, ctx);
        }

        if self.is_stdlib(specifier) {
            return ImportTarget::Stdlib;
        }

        // A bare specifier that looks like a build-tool alias rather than a package.
        // `@/components` and `~/lib` are tsconfig `paths` or bundler aliases; gdep
        // does not read tsconfig, so guessing here would invent missing dependencies.
        if specifier.starts_with("@/") || specifier.starts_with('~') {
            return ImportTarget::Unresolved {
                reason: format!("`{specifier}` looks like a path alias, not a package"),
            };
        }

        match package_name_of(specifier) {
            // A workspace sibling stays External: it really is a package dependency,
            // and calling it Internal would stop `unused-dep` seeing the declared
            // entry being used. The workspace exemption belongs in `missing-dep`,
            // which is the only check that would otherwise be wrong.
            Some(name) => ImportTarget::External(name),
            None => ImportTarget::Unresolved {
                reason: format!("`{specifier}` is not a usable package specifier"),
            },
        }
    }

    fn is_stdlib(&self, module: &str) -> bool {
        if let Some(rest) = module.strip_prefix("node:") {
            return !rest.is_empty();
        }
        let root = module.split('/').next().unwrap_or(module);
        NODE_BUILTINS.contains(&root)
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &NpmVersionOps
    }
}

/// The npm package a bare specifier belongs to: `@scope/pkg/deep` → `@scope/pkg`,
/// `pkg/deep` → `pkg`.
fn package_name_of(specifier: &str) -> Option<String> {
    let mut segments = specifier.split('/');
    let first = segments.next()?;
    if first.is_empty() {
        return None;
    }

    if first.starts_with('@') {
        // A scope alone is not a package.
        let second = segments.next()?;
        if first.len() < 2 || second.is_empty() {
            return None;
        }
        return Some(format!("{first}/{second}"));
    }

    Some(first.to_owned())
}

/// Resolves `./utils` against the importing file, trying Node's extension and
/// directory-index candidates in order.
fn resolve_relative(specifier: &str, from: &Utf8Path, ctx: &ProjectContext<'_>) -> ImportTarget {
    let base = from.parent().unwrap_or(Utf8Path::new(""));
    let joined = normalize(&base.join(specifier));

    // An exact hit, once extensions are stripped from both sides.
    let stripped = strip_module_extension(Utf8Path::new(&joined));
    for candidate in ctx.source_files {
        if strip_module_extension(candidate) == stripped {
            return ImportTarget::Internal(stripped);
        }
    }

    for extension in RESOLUTION_EXTENSIONS {
        let candidate = format!("{joined}.{extension}");
        if ctx
            .source_files
            .iter()
            .any(|file| file.as_str() == candidate)
        {
            return ImportTarget::Internal(strip_module_extension(Utf8Path::new(&candidate)));
        }
    }

    for extension in RESOLUTION_EXTENSIONS {
        let candidate = format!("{joined}/index.{extension}");
        if ctx
            .source_files
            .iter()
            .any(|file| file.as_str() == candidate)
        {
            return ImportTarget::Internal(strip_module_extension(Utf8Path::new(&candidate)));
        }
    }

    // A relative import is internal by definition even when it points at something
    // gdep does not parse — a stylesheet, a JSON fixture, an asset.
    ImportTarget::Internal(stripped)
}

/// Lexical path normalization. No filesystem access, so it works on paths that do
/// not exist.
fn normalize(path: &Utf8Path) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.as_str().split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Drops a module extension, including the compound `.d.ts`.
fn strip_module_extension(path: &Utf8Path) -> String {
    let raw = path.as_str();
    if let Some(base) = raw.strip_suffix(".d.ts") {
        return base.to_owned();
    }
    match path.extension() {
        Some(extension) if RESOLUTION_EXTENSIONS.contains(&extension) => {
            raw[..raw.len() - extension.len() - 1].to_owned()
        }
        _ => raw.to_owned(),
    }
}

/// Package names that are invoked as commands rather than imported, where the
/// command differs from the package name so a `scripts` scan alone misses them.
const KNOWN_TOOL_PACKAGES: &[&str] = &[
    "typescript",
    "npm-run-all",
    "rimraf",
    "shx",
    "shelljs",
    "husky",
    "lint-staged",
    "cross-env",
    "concurrently",
    "patch-package",
    "npm-check-updates",
    "c8",
    "nyc",
];

fn parse_package_json(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    let root: serde_json::Value = serde_json::from_str(text)?;
    let script_text = root
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .map(|scripts| {
            scripts
                .values()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let mut manifest = Manifest {
        package_name: root
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        deps: Vec::new(),
    };

    // `devDependencies` is not `Indirect`: dev dependencies are expected to be
    // imported, just from tests and tooling rather than shipped code.
    for (field, kind) in [
        ("dependencies", DepKind::Runtime),
        ("devDependencies", DepKind::Dev),
        ("peerDependencies", DepKind::Peer),
        ("optionalDependencies", DepKind::Optional),
    ] {
        let Some(entries) = root.get(field).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for (name, requirement) in entries {
            manifest.deps.push(DeclaredDep {
                name: name.clone(),
                requirement: requirement.as_str().unwrap_or_default().to_owned(),
                kind: if is_tooling(name, &script_text) {
                    DepKind::Tooling
                } else {
                    kind
                },
                declared_at: Provenance::new(path, find_key_line(text, name)),
            });
        }
    }

    Ok(manifest)
}

/// Whether a dependency is consumed without ever being imported.
///
/// Two shapes, both measured on real repositories rather than guessed at:
///
/// * `@types/*` packages are ambient declarations the TypeScript compiler reads.
///   Nothing imports them, ever, by construction.
/// * A package invoked from a `scripts` entry is a command, not a module. This is
///   how `eslint`, `rollup`, `prettier`, `vitest`, and `mocha` are used.
fn is_tooling(name: &str, script_text: &str) -> bool {
    if name.starts_with("@types/") {
        return true;
    }
    if KNOWN_TOOL_PACKAGES.contains(&name) {
        return true;
    }
    // Word-boundary match, so `vite` does not match `vitest`.
    script_text
        .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '@' || c == '/'))
        .any(|word| word == name)
}

/// Best-effort line number for a JSON key, so findings can cite one.
///
/// `serde_json` discards positions. Re-parsing with a span-preserving crate would
/// be the thorough fix; for a dependency name inside a manifest this is accurate in
/// practice, and a wrong line is visibly wrong rather than silently misleading.
fn find_key_line(text: &str, key: &str) -> Option<u32> {
    let needle = format!("\"{key}\"");
    text.lines()
        .position(|line| line.trim_start().starts_with(&needle))
        .map(|index| index as u32 + 1)
}

/// Parses `package-lock.json` v2/v3 into a resolved dependency graph.
///
/// Unlike `go.sum`, this genuinely is one: every installed package appears with its
/// exact version, and edges can be recovered by replaying Node's resolution
/// algorithm over the `packages` map.
fn parse_package_lock(text: &str) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
    let root: serde_json::Value = serde_json::from_str(text)?;

    // v1 lockfiles use a nested "dependencies" tree with no "packages" map. Rather
    // than half-support them, report nothing and let the check say Unavailable.
    let Some(packages) = root.get("packages").and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };

    let mut resolved = Vec::new();
    for (path, entry) in packages {
        // The "" entry is the root project, not a dependency.
        if path.is_empty() {
            continue;
        }
        let Some(name) = package_path_name(path) else {
            continue;
        };
        let version = entry
            .get("version")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();

        let mut dependencies = Vec::new();
        for field in ["dependencies", "optionalDependencies", "peerDependencies"] {
            let Some(entries) = entry.get(field).and_then(serde_json::Value::as_object) else {
                continue;
            };
            for dep_name in entries.keys() {
                if let Some(target) = resolve_npm_path(path, dep_name, packages) {
                    dependencies.push(target);
                }
            }
        }
        dependencies.sort();
        dependencies.dedup();

        resolved.push(ResolvedDep {
            key: path.clone(),
            name,
            version,
            dependencies,
        });
    }

    resolved.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(Some(resolved))
}

/// The package name encoded in a lockfile path: the part after the last
/// `node_modules/`, keeping a scope if present.
fn package_path_name(path: &str) -> Option<String> {
    let tail = path.rsplit_once("node_modules/").map(|(_, tail)| tail)?;
    if tail.is_empty() {
        return None;
    }
    Some(tail.to_owned())
}

/// Node's lookup: try `<from>/node_modules/<dep>`, then walk up one
/// `node_modules` level at a time. Returns the resolved package's key.
fn resolve_npm_path(
    from: &str,
    dep: &str,
    packages: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    let mut scope = from.to_owned();
    loop {
        let candidate = if scope.is_empty() {
            format!("node_modules/{dep}")
        } else {
            format!("{scope}/node_modules/{dep}")
        };
        if packages.contains_key(&candidate) {
            return Some(candidate);
        }
        match scope.rfind("/node_modules/") {
            Some(at) => scope.truncate(at),
            None if scope.is_empty() => return None,
            None => scope.clear(),
        }
    }
}

/// Extracts every import, re-export, `require`, and dynamic `import()`.
fn extract_js_imports(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let language = match path.extension() {
        Some("ts") | Some("mts") | Some("cts") => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        Some("tsx") => tree_sitter_typescript::LANGUAGE_TSX,
        _ => tree_sitter_javascript::LANGUAGE,
    };

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&language.into())
        .map_err(|error| anyhow::anyhow!("loading the grammar failed: {error}"))?;

    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect(tree.root_node(), text.as_bytes(), &mut imports);
    Ok(imports)
}

fn collect(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    match node.kind() {
        // `import x from "m"`, `export { x } from "m"`. A bare `export { x }` has
        // no source and must not produce an import.
        "import_statement" | "export_statement" => {
            if let Some(source_node) = node.child_by_field_name("source") {
                push_string(source_node, source, is_type_only(node, source), out);
            }
        }
        // `require("m")` and `import("m")`.
        "call_expression" => {
            if let Some(function) = node.child_by_field_name("function") {
                let is_loader = matches!(function.kind(), "import")
                    || function
                        .utf8_text(source)
                        .is_ok_and(|text| text == "require");
                if is_loader && let Some(arguments) = node.child_by_field_name("arguments") {
                    let mut cursor = arguments.walk();
                    for argument in arguments.children(&mut cursor) {
                        if argument.kind().ends_with("string") {
                            push_string(argument, source, false, out);
                            break;
                        }
                    }
                }
            }
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, out);
    }
}

/// `import type { T } from "m"` is erased at runtime but is still a real
/// dev-time dependency, so it is recorded with the flag set rather than dropped.
fn is_type_only(node: tree_sitter::Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == "type" && child.utf8_text(source).is_ok_and(|t| t == "type"))
}

fn push_string(node: tree_sitter::Node<'_>, source: &[u8], type_only: bool, out: &mut Vec<Import>) {
    let Ok(raw) = node.utf8_text(source) else {
        return;
    };
    let specifier = raw.trim_matches(['"', '\'', '`']);
    // A template literal with interpolation is a dynamic specifier; there is
    // nothing static to record.
    if specifier.is_empty() || specifier.contains("${") {
        return;
    }
    out.push(Import {
        raw: specifier.to_owned(),
        line: node.start_position().row as u32 + 1,
        type_only,
    });
}

/// Counts how many distinct versions of each package the resolved tree contains.
pub fn version_spread(resolved: &[ResolvedDep]) -> BTreeMap<&str, Vec<&str>> {
    let mut spread: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for dep in resolved {
        let versions = spread.entry(dep.name.as_str()).or_default();
        if !versions.contains(&dep.version.as_str()) {
            versions.push(dep.version.as_str());
        }
    }
    for versions in spread.values_mut() {
        versions.sort();
    }
    spread
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use gdep_core::model::Project;

    fn extract(file: &str, source: &str) -> Vec<Import> {
        extract_js_imports(Utf8Path::new(file), source).unwrap()
    }

    fn specifiers(file: &str, source: &str) -> Vec<String> {
        extract(file, source).into_iter().map(|i| i.raw).collect()
    }

    // --- package.json -----------------------------------------------------

    #[test]
    fn parses_all_four_dependency_fields() {
        let manifest = parse_package_json(
            Utf8Path::new("package.json"),
            r#"{
              "name": "app",
              "dependencies": { "react": "^18.0.0" },
              "devDependencies": { "vitest": "^1.0.0" },
              "peerDependencies": { "react-dom": "^18.0.0" },
              "optionalDependencies": { "fsevents": "^2.0.0" }
            }"#,
        )
        .unwrap();

        assert_eq!(manifest.package_name.as_deref(), Some("app"));
        let kinds: BTreeMap<&str, DepKind> = manifest
            .deps
            .iter()
            .map(|d| (d.name.as_str(), d.kind))
            .collect();
        assert_eq!(kinds["react"], DepKind::Runtime);
        assert_eq!(kinds["vitest"], DepKind::Dev);
        assert_eq!(kinds["react-dom"], DepKind::Peer);
        assert_eq!(kinds["fsevents"], DepKind::Optional);
    }

    #[test]
    fn a_manifest_with_no_dependencies_is_not_an_error() {
        let manifest =
            parse_package_json(Utf8Path::new("package.json"), r#"{"name":"x"}"#).unwrap();
        assert!(manifest.deps.is_empty());
    }

    #[test]
    fn malformed_json_is_an_error_not_a_silent_empty_result() {
        assert!(parse_package_json(Utf8Path::new("package.json"), "{ nope").is_err());
    }

    // --- package-lock.json ------------------------------------------------

    const LOCK: &str = r#"{
      "lockfileVersion": 3,
      "packages": {
        "": { "name": "app" },
        "node_modules/a": { "version": "1.0.0", "dependencies": { "shared": "^2.0.0" } },
        "node_modules/b": { "version": "1.0.0", "dependencies": { "shared": "^1.0.0" } },
        "node_modules/shared": { "version": "2.5.0" },
        "node_modules/b/node_modules/shared": { "version": "1.9.0" },
        "node_modules/@scope/pkg": { "version": "3.0.0" }
      }
    }"#;

    #[test]
    fn builds_a_resolved_tree_with_exact_versions() {
        let resolved = parse_package_lock(LOCK).unwrap().unwrap();
        let versions: BTreeMap<&str, &str> = resolved
            .iter()
            .map(|d| (d.name.as_str(), d.version.as_str()))
            .collect();
        assert_eq!(versions["a"], "1.0.0");
        assert_eq!(versions["@scope/pkg"], "3.0.0");
    }

    /// npm nests conflicting versions, so the same name appears twice. This is what
    /// makes version-conflict answerable for npm and impossible for Go.
    #[test]
    fn records_both_copies_of_a_duplicated_package() {
        let resolved = parse_package_lock(LOCK).unwrap().unwrap();
        let spread = version_spread(&resolved);
        assert_eq!(spread["shared"], vec!["1.9.0", "2.5.0"]);
    }

    /// Node resolution walks up: `b`'s `shared` is the nested copy, `a`'s is the
    /// hoisted one. Getting this wrong would collapse the two into one node.
    #[test]
    fn edges_follow_nodes_upward_lookup() {
        let resolved = parse_package_lock(LOCK).unwrap().unwrap();
        let by_name: BTreeMap<&str, &ResolvedDep> =
            resolved.iter().map(|d| (d.name.as_str(), d)).collect();
        assert_eq!(
            by_name["b"].dependencies,
            vec!["node_modules/b/node_modules/shared"]
        );
        assert_eq!(by_name["a"].dependencies, vec!["node_modules/shared"]);
    }

    #[test]
    fn the_root_entry_is_not_a_dependency() {
        let resolved = parse_package_lock(LOCK).unwrap().unwrap();
        assert!(resolved.iter().all(|d| !d.name.is_empty()));
    }

    /// v1 lockfiles have no "packages" map. Reporting nothing makes the check say
    /// Unavailable, which is true; a partial parse would be a confident wrong tree.
    #[test]
    fn a_v1_lockfile_yields_no_tree_rather_than_a_partial_one() {
        let v1 = r#"{"lockfileVersion": 1, "dependencies": {"a": {"version": "1.0.0"}}}"#;
        assert!(parse_package_lock(v1).unwrap().is_none());
    }

    // --- import extraction ------------------------------------------------

    #[test]
    fn extracts_esm_imports_and_re_exports() {
        let source = concat!(
            "import a from 'alpha';\n",
            "import { b } from \"beta\";\n",
            "import * as c from 'gamma';\n",
            "export { d } from 'delta';\n",
            "export * from 'epsilon';\n",
        );
        assert_eq!(
            specifiers("a.js", source),
            vec!["alpha", "beta", "gamma", "delta", "epsilon"]
        );
    }

    #[test]
    fn a_bare_export_without_a_source_is_not_an_import() {
        assert!(specifiers("a.js", "const x = 1;\nexport { x };\n").is_empty());
    }

    #[test]
    fn extracts_require_and_dynamic_import() {
        let source = "const a = require('alpha');\nconst b = await import('beta');\n";
        assert_eq!(specifiers("a.js", source), vec!["alpha", "beta"]);
    }

    #[test]
    fn extracts_typescript_type_only_imports_and_flags_them() {
        let imports = extract(
            "a.ts",
            "import type { T } from 'alpha';\nimport b from 'beta';\n",
        );
        assert_eq!(imports.len(), 2);
        assert!(imports[0].type_only, "import type should be flagged");
        assert!(!imports[1].type_only);
    }

    #[test]
    fn parses_tsx_with_jsx_syntax() {
        let source = "import React from 'react';\nexport const A = () => <div>hi</div>;\n";
        assert_eq!(specifiers("a.tsx", source), vec!["react"]);
    }

    /// The reason for a grammar rather than a regex.
    #[test]
    fn ignores_import_like_text_in_strings_and_comments() {
        let source = concat!(
            "import a from 'alpha';\n",
            "// import fake from 'commented';\n",
            "/* import fake from 'blocked'; */\n",
            "const s = \"import fake from 'quoted'\";\n",
        );
        assert_eq!(specifiers("a.js", source), vec!["alpha"]);
    }

    #[test]
    fn a_dynamic_template_specifier_is_skipped() {
        let source = "const m = await import(`./locales/${lang}.js`);\n";
        assert!(
            specifiers("a.js", source).is_empty(),
            "nothing static to record"
        );
    }

    #[test]
    fn reports_accurate_line_numbers() {
        let imports = extract("a.js", "\n\nimport a from 'alpha';\n");
        assert_eq!(imports[0].line, 3);
    }

    // --- resolution -------------------------------------------------------

    fn ctx_files(files: &[&str]) -> Vec<Utf8PathBuf> {
        files.iter().map(Utf8PathBuf::from).collect()
    }

    fn resolve(from: &str, specifier: &str, files: &[&str], declared: &[&str]) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::new(),
            language: Language::JavaScript,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: "^1.0.0".to_owned(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("package.json", Some(1)),
            })
            .collect();
        let source_files = ctx_files(files);
        let ctx = ProjectContext {
            project: &project,
            package_name: Some("app"),
            declared: &deps,
            sibling_packages: &[],
            source_files: &source_files,
        };
        let import = Import {
            raw: specifier.to_owned(),
            line: 1,
            type_only: false,
        };
        JavaScriptProvider.resolve_import(&import, Utf8Path::new(from), &ctx)
    }

    #[test]
    fn resolves_a_relative_import_with_an_implied_extension() {
        assert_eq!(
            resolve("src/a.ts", "./b", &["src/a.ts", "src/b.ts"], &[]),
            ImportTarget::Internal("src/b".to_owned())
        );
    }

    #[test]
    fn resolves_a_relative_import_to_a_directory_index() {
        assert_eq!(
            resolve(
                "src/a.ts",
                "./utils",
                &["src/a.ts", "src/utils/index.ts"],
                &[]
            ),
            ImportTarget::Internal("src/utils/index".to_owned())
        );
    }

    #[test]
    fn resolves_parent_relative_imports() {
        assert_eq!(
            resolve("src/deep/a.ts", "../b", &["src/deep/a.ts", "src/b.ts"], &[]),
            ImportTarget::Internal("src/b".to_owned())
        );
    }

    #[test]
    fn resolves_node_builtins_with_and_without_the_prefix() {
        assert_eq!(resolve("a.js", "fs", &[], &[]), ImportTarget::Stdlib);
        assert_eq!(
            resolve("a.js", "node:fs/promises", &[], &[]),
            ImportTarget::Stdlib
        );
        assert_eq!(resolve("a.js", "path", &[], &[]), ImportTarget::Stdlib);
    }

    #[test]
    fn resolves_a_bare_specifier_to_its_package() {
        assert_eq!(
            resolve("a.js", "lodash/fp", &[], &["lodash"]),
            ImportTarget::External("lodash".to_owned())
        );
    }

    #[test]
    fn resolves_a_scoped_package_including_a_subpath() {
        assert_eq!(
            resolve("a.js", "@scope/pkg/deep/thing", &[], &[]),
            ImportTarget::External("@scope/pkg".to_owned())
        );
    }

    /// A tsconfig `paths` alias is not a package. gdep does not read tsconfig, so
    /// guessing would invent a missing dependency on every aliased import.
    #[test]
    fn a_path_alias_is_unresolved_rather_than_a_guessed_package() {
        assert!(matches!(
            resolve("src/a.ts", "@/components/Button", &[], &[]),
            ImportTarget::Unresolved { .. }
        ));
        assert!(matches!(
            resolve("src/a.ts", "~/lib/x", &[], &[]),
            ImportTarget::Unresolved { .. }
        ));
    }

    #[test]
    fn a_node_subpath_import_is_internal() {
        assert_eq!(
            resolve("src/a.js", "#internal/db", &[], &[]),
            ImportTarget::Internal("#internal/db".to_owned())
        );
    }

    #[test]
    fn a_relative_import_of_an_unparsed_asset_is_still_internal() {
        assert_eq!(
            resolve("src/a.ts", "./styles.css", &["src/a.ts"], &[]),
            ImportTarget::Internal("src/styles.css".to_owned())
        );
    }

    // --- tooling classification -------------------------------------------

    /// Ambient type packages are read by the compiler and imported by nobody.
    #[test]
    fn types_packages_are_tooling() {
        assert!(is_tooling("@types/node", ""));
        assert!(is_tooling("@types/react", ""));
    }

    #[test]
    fn a_package_invoked_from_scripts_is_tooling() {
        assert!(is_tooling("eslint", "eslint . --fix"));
        assert!(is_tooling("rollup", "rollup -c && tsc"));
    }

    /// Word-boundary matching, or `vite` would be marked used by a `vitest` script.
    #[test]
    fn a_script_substring_does_not_count_as_a_match() {
        assert!(!is_tooling("vite", "vitest run"));
    }

    #[test]
    fn an_ordinary_library_is_not_tooling() {
        assert!(!is_tooling("lodash", "eslint . && vitest run"));
    }

    #[test]
    fn module_ids_are_files_not_directories() {
        let id = JavaScriptProvider.module_id_for_file(
            Utf8Path::new("src/components/Button.tsx"),
            "",
            "src/components",
        );
        assert_eq!(id, ModuleId::module("src/components/Button"));
    }

    #[test]
    fn declaration_files_lose_their_compound_extension() {
        assert_eq!(
            strip_module_extension(Utf8Path::new("types/api.d.ts")),
            "types/api"
        );
    }
}
