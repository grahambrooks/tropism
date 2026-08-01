//! Swift provider.
//!
//! `Package.swift` is the extreme case of a manifest that is code: it is not a
//! config file with an expression or two in it, it is a Swift program that
//! `swift build` compiles and runs to *produce* the manifest. It is parsed here
//! with the Swift grammar and never executed.
//!
//! Swift also has the cleanest answer to the import→package problem of any language
//! in this codebase, and it is worth stating plainly because every other provider
//! needs a curated table for it. **A Swift package declares the mapping itself.**
//! `import Logging` cannot be traced to the `swift-log` repository by any rule over
//! names — but the manifest says so, in the target that uses it:
//!
//! ```swift
//! .target(name: "ShopCore", dependencies: [
//!     .product(name: "Logging", package: "swift-log"),
//! ])
//! ```
//!
//! So a dependency is recorded under the *product* name — what `import` actually
//! writes — with one exception: a package no target takes a product from is
//! recorded under its own identity, because "declared and used by nothing" is
//! exactly the finding `unused-dep` exists for.

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};

pub struct SwiftProvider;

/// Modules supplied by the toolchain or the platform SDKs.
///
/// `PackageDescription` is on the list because `Package.swift` is itself a `.swift`
/// file that tropism walks, and it imports it.
const SYSTEM_MODULES: &[&str] = &[
    "Accelerate",
    "AppKit",
    "AVFoundation",
    "CloudKit",
    "Combine",
    "Contacts",
    "CoreData",
    "CoreFoundation",
    "CoreGraphics",
    "CoreImage",
    "CoreLocation",
    "CoreML",
    "CryptoKit",
    "Darwin",
    "Dispatch",
    "EventKit",
    "Foundation",
    "GameKit",
    "Glibc",
    "HealthKit",
    "MapKit",
    "Metal",
    "MetalKit",
    "Network",
    "Observation",
    "ObjectiveC",
    "os",
    "PackageDescription",
    "Photos",
    "RegexBuilder",
    "SceneKit",
    "Security",
    "SpriteKit",
    "StoreKit",
    "Swift",
    "SwiftUI",
    "Synchronization",
    "System",
    // Bundled with the toolchain since Swift 6; neither needs a package.
    "Testing",
    "UIKit",
    "WebKit",
    "XCTest",
];

struct SwiftVersionOps;

impl VersionOps for SwiftVersionOps {
    /// SwiftPM requires strict SemVer, so a numeric three-part comparison is the
    /// whole of it. `None` on a branch or revision pin, which has no ordering.
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

impl LanguageProvider for SwiftProvider {
    fn language(&self) -> Language {
        Language::Swift
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["Package.swift"]
    }

    fn lockfile_names(&self) -> &'static [&'static str] {
        &["Package.resolved"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["swift"]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        parse_package_swift(path, text)
    }

    /// `Package.resolved` pins a version per package and records no edges at all —
    /// the same shape as `gradle.lockfile`, and the same consequence.
    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        _text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        Ok(None)
    }

    fn resolved_tree_note(&self) -> Option<&'static str> {
        Some(
            "Package.resolved is a flat list of pinned packages with no dependency \
             edges, and SwiftPM resolves to one version per package, so it can \
             answer neither a diamond nor a conflict question",
        )
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_swift_imports(path, text)
    }

    /// A Swift module is a *target*, and a target owns one directory under
    /// `Sources/`. This is the only language here whose module boundary is neither
    /// the directory, the file, nor a declaration in the source — it is a name in
    /// the manifest, and the convention that maps it to a directory is what the
    /// path lookup relies on.
    fn module_id_for_file(&self, path: &Utf8Path, _text: &str, default_id: &str) -> ModuleId {
        match target_of(path) {
            Some((name, false)) => ModuleId::module(name),
            // A test target is a separate module that exists to import the one it
            // tests, and `@testable import` is that made explicit.
            Some((name, true)) => ModuleId::external_test(name),
            None => ModuleId::module(default_id),
        }
    }

    fn resolve_import(
        &self,
        import: &Import,
        _from: &Utf8Path,
        ctx: &ProjectContext<'_>,
    ) -> ImportTarget {
        let module = import.raw.as_str();

        // A target in this package. Checked first: a local module shadows a system
        // one, and Swift resolves it that way too.
        if ctx.known_modules.contains(module) {
            return ImportTarget::Internal(module.to_owned());
        }

        if self.is_stdlib(module) {
            return ImportTarget::Stdlib;
        }

        // A product this package depends on. Exact, because the manifest recorded
        // the product name and `import` writes exactly that.
        if let Some(dep) = ctx.declared.iter().find(|dep| dep.name == module) {
            return ImportTarget::External(dep.name.clone());
        }
        if let Some(sibling) = ctx.sibling_packages.iter().find(|name| *name == module) {
            return ImportTarget::External(sibling.clone());
        }

        ImportTarget::External(module.to_owned())
    }

    fn resolve_cross_project(
        &self,
        import: &Import,
        target: &ProjectContext<'_>,
    ) -> Option<String> {
        target
            .known_modules
            .contains(&import.raw)
            .then(|| import.raw.clone())
    }

    fn is_stdlib(&self, module: &str) -> bool {
        SYSTEM_MODULES.contains(&module)
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &SwiftVersionOps
    }
}

/// The target a source file belongs to, and whether it is a test target.
///
/// SwiftPM's convention is `Sources/<Target>/…` and `Tests/<Target>/…`. A target
/// may override its path, which tropism cannot see without evaluating the manifest —
/// such a file falls back to the directory, which is less precise and never wrong.
fn target_of(path: &Utf8Path) -> Option<(String, bool)> {
    let parts: Vec<&str> = path.as_str().split('/').collect();
    parts.iter().enumerate().find_map(|(index, part)| {
        let is_test = *part == "Tests";
        (is_test || *part == "Sources")
            .then(|| parts.get(index + 1))
            .flatten()
            .map(|name| ((*name).to_owned(), is_test))
    })
}

// --- Package.swift -----------------------------------------------------------

/// A package dependency, before its products are known.
struct PackageRef {
    identity: String,
    requirement: String,
    line: u32,
}

/// A product some target depends on.
struct ProductRef {
    product: String,
    package: String,
    test_only: bool,
    line: u32,
}

/// Parses `Package.swift`.
///
/// Walks the syntax tree for the three call shapes that carry dependency
/// information — `.package(url:)`, the target constructors, and `.product(name:package:)`
/// inside a target's `dependencies:` — and ignores everything else. A manifest that
/// computes a name in a loop contributes nothing rather than a guess, which is the
/// rule for every manifest that is a program.
fn parse_package_swift(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_swift::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Swift grammar failed: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let source = text.as_bytes();
    let mut packages: Vec<PackageRef> = Vec::new();
    let mut products: Vec<ProductRef> = Vec::new();
    let mut targets: Vec<String> = Vec::new();
    let mut package_name: Option<String> = None;

    walk_calls(tree.root_node(), &mut |node| {
        let Some(callee) = callee_name(node, source) else {
            return;
        };
        let line = node.start_position().row as u32 + 1;

        match callee.as_str() {
            "Package" => {
                package_name = argument(node, "name", source).and_then(|n| string_value(n, source));
            }
            "package" => {
                // `.package(url:)`, `.package(path:)`, or `.package(id:)`.
                let identity = ["url", "path", "id", "name"]
                    .iter()
                    .find_map(|label| argument(node, label, source))
                    .and_then(|value| string_value(value, source))
                    .map(|value| package_identity(&value));
                if let Some(identity) = identity.filter(|identity| !identity.is_empty()) {
                    packages.push(PackageRef {
                        identity,
                        requirement: ["from", "exact", "branch", "revision"]
                            .iter()
                            .find_map(|label| argument(node, label, source))
                            .and_then(|value| string_value(value, source))
                            .unwrap_or_default(),
                        line,
                    });
                }
            }
            "target" | "executableTarget" | "testTarget" | "macro" | "systemLibrary" | "plugin" => {
                let Some(name) =
                    argument(node, "name", source).and_then(|n| string_value(n, source))
                else {
                    return;
                };
                // Every target is a module of this package, test or not; what the
                // flag decides is whether the products it pulls in are Dev.
                let test_only = callee == "testTarget";
                targets.push(name.clone());

                let Some(dependencies) = argument(node, "dependencies", source) else {
                    return;
                };
                collect_target_dependencies(dependencies, source, test_only, line, &mut products);
            }
            _ => {}
        }
    });

    // A bare string in a target's `dependencies:` names either another target in
    // this package or a package whose product shares its name. Only the second is a
    // dependency; the first is an internal edge the module graph already has.
    products.retain(|product| !targets.contains(&product.product));

    let mut deps: Vec<DeclaredDep> = Vec::new();
    for package in &packages {
        let from_package: Vec<&ProductRef> = products
            .iter()
            .filter(|product| {
                product.package == package.identity || product.product == package.identity
            })
            .collect();

        if from_package.is_empty() {
            // Declared and used by no target. Recorded under the package's own
            // identity so it is reported unused, which is what it is.
            deps.push(DeclaredDep {
                name: package.identity.clone(),
                requirement: package.requirement.clone(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new(path, Some(package.line)),
            });
            continue;
        }

        for product in from_package {
            deps.push(DeclaredDep {
                name: product.product.clone(),
                requirement: package.requirement.clone(),
                kind: if product.test_only {
                    DepKind::Dev
                } else {
                    DepKind::Runtime
                },
                declared_at: Provenance::new(path, Some(product.line)),
            });
        }
    }

    // A product taken from a package that was never declared: keep it, so the code
    // importing it is not reported as importing something undeclared. SwiftPM
    // rejects this at build time, which is a different tool's job.
    for product in &products {
        if !deps.iter().any(|dep| dep.name == product.product) {
            deps.push(DeclaredDep {
                name: product.product.clone(),
                requirement: String::new(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new(path, Some(product.line)),
            });
        }
    }

    deps.sort_by(|a, b| (&a.name, a.kind).cmp(&(&b.name, b.kind)));
    deps.dedup_by(|a, b| a.name == b.name);

    Ok(Manifest { deps, package_name })
}

/// Reads the items of a target's `dependencies:` array.
fn collect_target_dependencies(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    test_only: bool,
    target_line: u32,
    out: &mut Vec<ProductRef>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let line = child.start_position().row as u32 + 1;

        // `.product(name: "Logging", package: "swift-log")` — the manifest stating
        // the module→package mapping outright.
        if child.kind() == "call_expression" {
            let Some(callee) = callee_name(child, source) else {
                continue;
            };
            if callee == "product" {
                let product = argument(child, "name", source).and_then(|n| string_value(n, source));
                let package =
                    argument(child, "package", source).and_then(|n| string_value(n, source));
                if let Some(product) = product {
                    out.push(ProductRef {
                        package: package.unwrap_or_else(|| product.clone()),
                        product,
                        test_only,
                        line,
                    });
                }
            } else if (callee == "target" || callee == "byName")
                && let Some(name) =
                    argument(child, "name", source).and_then(|n| string_value(n, source))
            {
                out.push(ProductRef {
                    package: name.clone(),
                    product: name,
                    test_only,
                    line,
                });
            }
            continue;
        }

        // A bare `"ShopCore"`: another target, or a package whose product shares
        // its name. Which one is decided after every target is known.
        if let Some(name) = string_value(child, source) {
            out.push(ProductRef {
                package: name.clone(),
                product: name,
                test_only,
                line: if line == 0 { target_line } else { line },
            });
        }
    }
}

/// `https://github.com/apple/swift-log.git` → `swift-log`.
///
/// SwiftPM calls this the package identity, and it is what `.product(package:)`
/// names.
fn package_identity(location: &str) -> String {
    location
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(location)
        .trim_end_matches(".git")
        .to_owned()
}

fn walk_calls(node: tree_sitter::Node<'_>, visit: &mut impl FnMut(tree_sitter::Node<'_>)) {
    if node.kind() == "call_expression" {
        visit(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_calls(child, visit);
    }
}

/// The name of the function a `call_expression` calls.
///
/// `Package(…)` is a plain identifier; `.package(…)` is a prefix expression whose
/// identifier follows the dot. Both forms appear in every manifest.
fn callee_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let head = node.child(0)?;
    match head.kind() {
        "simple_identifier" => head.utf8_text(source).ok().map(str::to_owned),
        "prefix_expression" => {
            let mut cursor = head.walk();
            head.named_children(&mut cursor)
                .find(|child| child.kind() == "simple_identifier")
                .and_then(|child| child.utf8_text(source).ok())
                .map(str::to_owned)
        }
        _ => None,
    }
}

/// The value of a labelled argument: `name:` in `.target(name: "X")`.
fn argument<'a>(
    call: tree_sitter::Node<'a>,
    label: &str,
    source: &[u8],
) -> Option<tree_sitter::Node<'a>> {
    let suffix = child_of_kind(call, "call_suffix")?;
    let arguments = child_of_kind(suffix, "value_arguments")?;

    let mut cursor = arguments.walk();
    for argument in arguments.named_children(&mut cursor) {
        if argument.kind() != "value_argument" {
            continue;
        }
        let matches = child_of_kind(argument, "value_argument_label")
            .and_then(|node| node.utf8_text(source).ok())
            .is_some_and(|text| text == label);
        if !matches {
            continue;
        }
        // The value is the last named child; the label is the first.
        let mut inner = argument.walk();
        let value = argument
            .named_children(&mut inner)
            .filter(|child| child.kind() != "value_argument_label")
            .last();
        if let Some(value) = value {
            return Some(value);
        }
    }
    None
}

fn child_of_kind<'a>(node: tree_sitter::Node<'a>, kind: &str) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// The literal text of a string, or `None` when it is interpolated.
///
/// `.package(url: "\(base)/swift-log.git")` names a repository that cannot be known
/// without running the manifest, and inventing one would put a package that does
/// not exist into a report.
fn string_value(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "line_string_literal" {
        return None;
    }
    let mut cursor = node.walk();
    let mut text = String::new();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "line_str_text" => text.push_str(child.utf8_text(source).ok()?),
            "interpolated_expression" | "interpolation" => return None,
            _ => {}
        }
    }
    (!text.is_empty()).then_some(text)
}

// --- import extraction -------------------------------------------------------

/// Extracts every `import` from one Swift file.
///
/// Swift imports a *module*, so `import struct Shop.Order` and `@testable import
/// ShopCore` both reduce to the first identifier — the only granularity resolution
/// needs.
fn extract_swift_imports(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_swift::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Swift grammar failed: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect_imports(tree.root_node(), text.as_bytes(), &mut imports);
    Ok(imports)
}

fn collect_imports(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    if node.kind() == "import_declaration" {
        let mut cursor = node.walk();
        let module = node
            .named_children(&mut cursor)
            .find(|child| child.kind() == "identifier")
            .and_then(|identifier| {
                let mut inner = identifier.walk();
                identifier
                    .named_children(&mut inner)
                    .find(|child| child.kind() == "simple_identifier")
                    .or(Some(identifier))
            })
            .and_then(|node| node.utf8_text(source).ok());

        if let Some(module) = module {
            out.push(Import::statement(
                module.to_owned(),
                node.start_position().row as u32 + 1,
            ));
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_imports(child, source, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;
    use tropism_core::model::Project;

    fn modules(source: &str) -> Vec<String> {
        extract_swift_imports(Utf8Path::new("A.swift"), source)
            .unwrap()
            .into_iter()
            .map(|import| import.raw)
            .collect()
    }

    fn manifest(text: &str) -> Manifest {
        parse_package_swift(Utf8Path::new("Package.swift"), text).unwrap()
    }

    const FULL: &str = r#"// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Shop",
    dependencies: [
        .package(url: "https://github.com/apple/swift-log.git", from: "1.5.0"),
        .package(url: "https://github.com/apple/swift-argument-parser.git", from: "1.4.0"),
    ],
    targets: [
        .target(name: "ShopCore", dependencies: [
            .product(name: "Logging", package: "swift-log"),
        ]),
        .executableTarget(name: "ShopCLI", dependencies: ["ShopCore"]),
        .testTarget(name: "ShopCoreTests", dependencies: ["ShopCore"]),
    ]
)
"#;

    // --- Package.swift ----------------------------------------------------

    #[test]
    fn reads_the_package_name() {
        assert_eq!(manifest(FULL).package_name.as_deref(), Some("Shop"));
    }

    /// The mapping no other ecosystem supplies: `import Logging` is `swift-log`,
    /// and the manifest says so.
    #[test]
    fn a_dependency_is_recorded_under_the_product_name_that_code_imports() {
        let manifest = manifest(FULL);
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Logging"), "got {names:?}");
        assert_eq!(
            manifest
                .deps
                .iter()
                .find(|d| d.name == "Logging")
                .unwrap()
                .requirement,
            "1.5.0"
        );
    }

    /// A package no target takes a product from is exactly what `unused-dep` is
    /// for, so it keeps its own identity rather than vanishing.
    #[test]
    fn a_package_with_no_product_in_use_is_recorded_under_its_identity() {
        let manifest = manifest(FULL);
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"swift-argument-parser"), "got {names:?}");
    }

    /// A bare string naming another target in the same package is an internal edge,
    /// not a dependency.
    #[test]
    fn a_sibling_target_is_not_a_dependency() {
        let manifest = manifest(FULL);
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert!(!names.contains(&"ShopCore"), "got {names:?}");
    }

    #[test]
    fn a_product_used_only_by_a_test_target_is_a_dev_dependency() {
        let manifest = manifest(
            r#"let package = Package(
    name: "Shop",
    dependencies: [.package(url: "https://github.com/apple/swift-testing.git", from: "0.1.0")],
    targets: [
        .testTarget(name: "ShopTests", dependencies: [.product(name: "SwiftTesting", package: "swift-testing")]),
    ]
)
"#,
        );
        let dep = manifest
            .deps
            .iter()
            .find(|d| d.name == "SwiftTesting")
            .unwrap();
        assert_eq!(dep.kind, DepKind::Dev);
    }

    #[test]
    fn a_git_url_becomes_the_package_identity() {
        assert_eq!(
            package_identity("https://github.com/apple/swift-log.git"),
            "swift-log"
        );
        assert_eq!(package_identity("../local-package"), "local-package");
    }

    /// The manifest is a program, and an interpolated URL names a repository that
    /// cannot be known without running it.
    #[test]
    fn an_interpolated_url_is_skipped_rather_than_guessed() {
        let manifest = manifest(
            "let base = \"https://github.com/x\"\nlet package = Package(name: \"S\", dependencies: [.package(url: \"\\(base)/y.git\", from: \"1.0.0\")], targets: [])\n",
        );
        assert!(manifest.deps.is_empty(), "got {:?}", manifest.deps);
    }

    // --- import extraction ------------------------------------------------

    #[test]
    fn extracts_plain_submodule_and_testable_imports() {
        assert_eq!(
            modules("import Foundation\nimport struct Shop.Order\n@testable import ShopCore\n"),
            vec!["Foundation", "Shop", "ShopCore"]
        );
    }

    /// The reason for a grammar instead of a regex.
    #[test]
    fn ignores_imports_in_strings_and_comments() {
        let source = concat!(
            "import Foundation\n",
            "// import FakeCommented\n",
            "/* import FakeBlock */\n",
            "let s = \"import FakeQuoted\"\n",
        );
        assert_eq!(modules(source), vec!["Foundation"]);
    }

    #[test]
    fn reports_accurate_line_numbers() {
        let imports = extract_swift_imports(
            Utf8Path::new("A.swift"),
            "import Foundation\n\nimport Logging\n",
        )
        .unwrap();
        assert_eq!(imports[0].line, 1);
        assert_eq!(imports[1].line, 3);
    }

    // --- module identity --------------------------------------------------

    #[test]
    fn a_module_is_the_target_directory_under_sources() {
        let id = SwiftProvider.module_id_for_file(
            Utf8Path::new("Sources/ShopCore/Order.swift"),
            "",
            "Sources/ShopCore",
        );
        assert_eq!(id.name, "ShopCore");
        assert!(!id.kind.is_test());
    }

    #[test]
    fn a_target_under_tests_is_an_external_test_module() {
        let id = SwiftProvider.module_id_for_file(
            Utf8Path::new("Tests/ShopCoreTests/OrderTests.swift"),
            "",
            "Tests/ShopCoreTests",
        );
        assert_eq!(id.name, "ShopCoreTests");
        assert!(id.kind.is_test());
    }

    /// A target with a custom `path:` cannot be found by convention, so the
    /// directory is used — less precise, never wrong.
    #[test]
    fn a_file_outside_the_conventional_layout_falls_back_to_its_directory() {
        let id = SwiftProvider.module_id_for_file(Utf8Path::new("custom/A.swift"), "", "custom");
        assert_eq!(id.name, "custom");
    }

    // --- resolution -------------------------------------------------------

    fn resolve(raw: &str, declared: &[&str], modules: &[&str]) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::from("."),
            language: Language::Swift,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: String::new(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("Package.swift", Some(1)),
            })
            .collect();
        let known: BTreeSet<String> = modules.iter().map(|m| (*m).to_owned()).collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: Some("Shop"),
            declared: &deps,
            sibling_packages: &[],
            known_modules: &known,
            source_files: &[],
        };
        SwiftProvider.resolve_import(
            &Import::statement(raw, 1),
            Utf8Path::new("Sources/ShopCore/A.swift"),
            &ctx,
        )
    }

    #[test]
    fn resolves_system_modules() {
        assert_eq!(resolve("Foundation", &[], &[]), ImportTarget::Stdlib);
        assert_eq!(resolve("XCTest", &[], &[]), ImportTarget::Stdlib);
        assert_eq!(
            resolve("PackageDescription", &[], &[]),
            ImportTarget::Stdlib,
            "Package.swift is itself a source file tropism walks"
        );
    }

    #[test]
    fn resolves_a_target_in_this_package() {
        assert_eq!(
            resolve("ShopCore", &[], &["ShopCore"]),
            ImportTarget::Internal("ShopCore".to_owned())
        );
    }

    #[test]
    fn resolves_a_declared_product() {
        assert_eq!(
            resolve("Logging", &["Logging"], &[]),
            ImportTarget::External("Logging".to_owned())
        );
    }

    #[test]
    fn an_undeclared_module_is_external_so_it_is_reported_missing() {
        assert_eq!(
            resolve("Alamofire", &[], &[]),
            ImportTarget::External("Alamofire".to_owned())
        );
    }

    // --- lockfile ---------------------------------------------------------

    #[test]
    fn package_resolved_is_not_treated_as_a_resolved_tree() {
        let parsed = SwiftProvider
            .parse_lockfile(Utf8Path::new("Package.resolved"), "{\"pins\":[]}")
            .unwrap();
        assert!(parsed.is_none());
        let note = SwiftProvider.resolved_tree_note().unwrap();
        assert!(
            note.contains("Package.resolved") && note.contains("edges"),
            "{note}"
        );
    }
}
