//! C# / .NET provider.
//!
//! Two things here are unlike the first three languages.
//!
//! The manifest is named after the project — `MyApp.csproj` — rather than by
//! convention, which is why the trait grew `manifest_extensions`.
//!
//! And a `using` names a *namespace*, not a package. `using Xunit;` comes from the
//! `xunit` package and `using NUnit.Framework;` from `NUnit`, while
//! `using MyApp.Services;` is the project's own code. Telling those apart is the
//! import→package problem in its sharpest form so far, and it is why the trait
//! grew `known_modules`: the namespaces a project declares are the only reliable
//! way to recognise its own code.

use std::collections::BTreeSet;

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};

pub struct CSharpProvider;

/// Namespace roots provided by the framework, which need no package reference.
///
/// `System.*` is the BCL. In a modern SDK-style project the whole of it ships with
/// the runtime; in older projects some pieces were separate packages, so treating
/// them as framework can hide a genuinely missing reference. That trade is
/// deliberate: the alternative reports a missing dependency on `System.Linq`.
const FRAMEWORK_ROOTS: &[&str] = &["System", "Microsoft.CSharp", "Microsoft.VisualBasic"];

/// Namespaces whose package name is not a prefix of the namespace.
///
/// The residue left after longest-prefix matching. Kept small and per-ecosystem, as
/// `design/03-language-providers.md` requires — a wrong entry produces a confident
/// false finding, so anything uncertain is left `Unresolved` instead.
const NAMESPACE_TO_PACKAGE: &[(&str, &str)] = &[
    ("Xunit", "xunit"),
    ("NUnit.Framework", "NUnit"),
    ("NSubstitute", "NSubstitute"),
    ("FluentAssertions", "FluentAssertions"),
    ("Newtonsoft.Json", "Newtonsoft.Json"),
    ("Serilog", "Serilog"),
    ("AutoMapper", "AutoMapper"),
    ("Dapper", "Dapper"),
    ("MediatR", "MediatR"),
    ("Moq", "Moq"),
    ("Polly", "Polly"),
];

struct NuGetVersionOps;

impl VersionOps for NuGetVersionOps {
    /// NuGet versions are SemVer with a four-part variant, but no current check
    /// needs an ordering — duplicate detection compares for equality.
    fn compare(&self, _a: &str, _b: &str) -> Option<std::cmp::Ordering> {
        None
    }

    fn satisfies(&self, _version: &str, _requirement: &str) -> Option<bool> {
        None
    }
}

impl LanguageProvider for CSharpProvider {
    fn language(&self) -> Language {
        Language::CSharp
    }

    /// Central Package Management puts versions in `Directory.Packages.props`, but
    /// that file declares no dependencies of its own — it is a version catalogue,
    /// like Cargo's `[workspace.dependencies]`. Treating it as a manifest would
    /// report every entry unused.
    fn manifest_names(&self) -> &'static [&'static str] {
        &[]
    }

    fn manifest_extensions(&self) -> &'static [&'static str] {
        &["csproj"]
    }

    /// `packages.lock.json` is opt-in (`RestorePackagesWithLockFile`) and usually
    /// absent, so resolved-tree checks are unavailable for most .NET repositories.
    fn lockfile_names(&self) -> &'static [&'static str] {
        &["packages.lock.json"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["cs"]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        parse_csproj(path, text)
    }

    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        parse_packages_lock(text)
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_csharp_usings(path, text)
    }

    /// A C# module is the namespace the file declares — not its path.
    ///
    /// .NET projects routinely put `MyApp.Services` in `Services/`, but nothing
    /// enforces it, and the namespace is what `using` statements actually name. This
    /// is the fourth distinct file→module strategy across four languages, which is
    /// the clearest evidence the mapping belongs to the provider.
    fn module_id_for_file(&self, path: &Utf8Path, text: &str, default_id: &str) -> ModuleId {
        let name = namespace_of(text).unwrap_or_else(|| default_id.to_owned());

        // Test projects are separate assemblies that reference the code under test,
        // so they can never form a cycle with it.
        let is_test = path
            .as_str()
            .split('/')
            .any(|segment| segment.ends_with(".Tests") || segment.ends_with(".Test"));
        if is_test {
            ModuleId::external_test(name)
        } else {
            ModuleId::module(name)
        }
    }

    fn resolve_import(
        &self,
        import: &Import,
        _from: &Utf8Path,
        ctx: &ProjectContext<'_>,
    ) -> ImportTarget {
        let namespace = import.raw.as_str();

        if is_framework(namespace) {
            return ImportTarget::Stdlib;
        }

        // 1. A namespace this project declares. Longest prefix wins, because
        //    `using MyApp.Services.Impl` may refer to the `MyApp.Services` module.
        if let Some(module) =
            longest_prefix(namespace, ctx.known_modules.iter().map(String::as_str))
        {
            return ImportTarget::Internal(module);
        }

        // 2. The project's own root namespace, for a namespace no file declares
        //    directly — a nested type, say.
        if let Some(root) = ctx.package_name
            && is_namespace_prefix(namespace, root)
        {
            return ImportTarget::Internal(root.to_owned());
        }

        // 3. A declared PackageReference or a sibling project. Authoritative.
        let candidates = ctx
            .declared
            .iter()
            .map(|dep| dep.name.as_str())
            .chain(ctx.sibling_packages.iter().map(String::as_str));
        if let Some(package) = longest_prefix(namespace, candidates) {
            return ImportTarget::External(package);
        }

        // 4. The curated residue.
        if let Some((_, package)) = NAMESPACE_TO_PACKAGE
            .iter()
            .filter(|(ns, _)| is_namespace_prefix(namespace, ns))
            .max_by_key(|(ns, _)| ns.len())
        {
            return ImportTarget::External((*package).to_owned());
        }

        // A namespace matching nothing is more likely a project reference tropism could
        // not see than an undeclared package, so it stays unresolved rather than
        // becoming a confident missing-dependency finding.
        ImportTarget::Unresolved {
            reason: format!("`{namespace}` matches no declared package or known namespace"),
        }
    }

    fn is_stdlib(&self, module: &str) -> bool {
        is_framework(module)
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &NuGetVersionOps
    }
}

fn is_framework(namespace: &str) -> bool {
    FRAMEWORK_ROOTS
        .iter()
        .any(|root| is_namespace_prefix(namespace, root))
}

/// Whether `prefix` is a dotted-segment prefix of `namespace`.
///
/// Guards against `SystemX` matching `System`, and is case-insensitive because
/// NuGet package ids are.
fn is_namespace_prefix(namespace: &str, prefix: &str) -> bool {
    if namespace.len() < prefix.len() {
        return false;
    }
    if !namespace[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return false;
    }
    namespace.len() == prefix.len() || namespace.as_bytes()[prefix.len()] == b'.'
}

fn longest_prefix<'a>(
    namespace: &str,
    candidates: impl Iterator<Item = &'a str>,
) -> Option<String> {
    candidates
        .filter(|candidate| is_namespace_prefix(namespace, candidate))
        .max_by_key(|candidate| candidate.len())
        .map(str::to_owned)
}

/// The namespace a file declares. Handles both block and file-scoped forms.
fn namespace_of(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        let Some(rest) = line.strip_prefix("namespace ") else {
            continue;
        };
        let name = rest.trim().trim_end_matches(['{', ';']).trim();
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    None
}

/// Parses a `.csproj`.
///
/// `<ProjectReference>` becomes a declared dependency named after the referenced
/// project, so a solution's internal edges are visible to the rule engine exactly
/// as package references are.
fn parse_csproj(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(text);
    // Unbalanced tags must fail the project rather than yield a silently empty
    // dependency list, which would read as "this project declares nothing".
    reader.config_mut().check_end_names = true;
    let mut deps: Vec<DeclaredDep> = Vec::new();
    let mut root_namespace: Option<String> = None;
    let mut assembly_name: Option<String> = None;
    let mut current_element: Option<String> = None;
    let mut buffer = String::new();
    let mut depth: i32 = 0;

    loop {
        match reader.read_event() {
            Err(error) => anyhow::bail!("{path}: {error}"),
            Ok(Event::Eof) => {
                // check_end_names catches a *mismatched* close tag, but a tag simply
                // left open runs to EOF without complaint.
                if depth != 0 {
                    anyhow::bail!("{path}: {depth} unclosed element(s)");
                }
                break;
            }
            Ok(event @ (Event::Start(_) | Event::Empty(_))) => {
                let is_start = matches!(event, Event::Start(_));
                if is_start {
                    depth += 1;
                }
                let element = match &event {
                    Event::Start(element) | Event::Empty(element) => element,
                    _ => unreachable!(),
                };
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();

                let attribute = |key: &str| -> Option<String> {
                    element.attributes().flatten().find_map(|attr| {
                        (attr.key.local_name().as_ref() == key.as_bytes())
                            .then(|| String::from_utf8_lossy(&attr.value).into_owned())
                    })
                };

                match name.as_str() {
                    "PackageReference" => {
                        if let Some(include) = attribute("Include") {
                            // PrivateAssets="all" marks a Roslyn analyzer or build
                            // task. It participates in the build and is never
                            // referenced from code, so it is Tooling — the same
                            // shape as an npm package invoked from `scripts`.
                            let kind = if attribute("PrivateAssets")
                                .is_some_and(|value| value.eq_ignore_ascii_case("all"))
                            {
                                DepKind::Tooling
                            } else {
                                DepKind::Runtime
                            };
                            deps.push(DeclaredDep {
                                requirement: attribute("Version").unwrap_or_default(),
                                declared_at: Provenance::new(path, find_line(text, &include)),
                                name: include,
                                kind,
                            });
                        }
                    }
                    "ProjectReference" => {
                        if let Some(include) = attribute("Include") {
                            let referenced = project_name_of(&include);
                            deps.push(DeclaredDep {
                                requirement: String::new(),
                                declared_at: Provenance::new(path, find_line(text, &include)),
                                name: referenced,
                                kind: DepKind::Runtime,
                            });
                        }
                    }
                    _ => current_element = Some(name),
                }
                buffer.clear();
            }
            Ok(Event::Text(text_event)) => {
                buffer.push_str(&text_event.decode().unwrap_or_default());
            }
            Ok(Event::End(element)) => {
                depth -= 1;
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                if current_element.as_deref() == Some(name.as_str()) {
                    match name.as_str() {
                        "RootNamespace" => root_namespace = Some(buffer.trim().to_owned()),
                        "AssemblyName" => assembly_name = Some(buffer.trim().to_owned()),
                        _ => {}
                    }
                }
                current_element = None;
                buffer.clear();
            }
            Ok(_) => {}
        }
    }

    // Default root namespace is the project file's own name.
    let fallback = path.file_stem().map(str::to_owned);
    deps.sort_by(|a, b| (&a.name, a.kind).cmp(&(&b.name, b.kind)));
    deps.dedup_by(|a, b| a.name == b.name && a.kind == b.kind);

    Ok(Manifest {
        package_name: root_namespace.or(assembly_name).or(fallback),
        deps,
    })
}

/// `../Domain/Domain.csproj` → `Domain`.
fn project_name_of(include: &str) -> String {
    include
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .and_then(|file| file.strip_suffix(".csproj"))
        .map(str::to_owned)
        .unwrap_or_else(|| include.to_owned())
}

fn find_line(text: &str, needle: &str) -> Option<u32> {
    text.lines()
        .position(|line| line.contains(needle))
        .map(|index| index as u32 + 1)
}

/// Parses `packages.lock.json` into a resolved graph.
///
/// A genuinely resolved tree when present: every package carries its selected
/// version and its edges.
fn parse_packages_lock(text: &str) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
    let root: serde_json::Value = serde_json::from_str(text)?;
    let Some(frameworks) = root
        .get("dependencies")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(None);
    };

    let mut resolved: Vec<ResolvedDep> = Vec::new();
    let mut seen = BTreeSet::new();

    for packages in frameworks.values() {
        let Some(packages) = packages.as_object() else {
            continue;
        };
        for (name, entry) in packages {
            let version = entry
                .get("resolved")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let key = format!("{name} {version}");
            if !seen.insert(key.clone()) {
                continue;
            }

            let mut dependencies: Vec<String> = entry
                .get("dependencies")
                .and_then(serde_json::Value::as_object)
                .map(|edges| {
                    edges
                        .keys()
                        .filter_map(|dep| {
                            packages
                                .get(dep)
                                .and_then(|target| target.get("resolved"))
                                .and_then(serde_json::Value::as_str)
                                .map(|version| format!("{dep} {version}"))
                        })
                        .collect()
                })
                .unwrap_or_default();
            dependencies.sort();

            resolved.push(ResolvedDep {
                key,
                name: name.clone(),
                version,
                dependencies,
            });
        }
    }

    resolved.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok((!resolved.is_empty()).then_some(resolved))
}

fn extract_csharp_usings(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the C# grammar failed: {error}"))?;

    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect(tree.root_node(), text.as_bytes(), &mut imports);
    imports.sort_by(|a, b| (a.line, &a.raw).cmp(&(b.line, &b.raw)));
    imports.dedup_by(|a, b| a.line == b.line && a.raw == b.raw);
    Ok(imports)
}

fn collect(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    if node.kind() == "using_directive"
        && let Ok(raw) = node.utf8_text(source)
        && let Some(namespace) = using_namespace(raw)
    {
        out.push(Import::statement(
            namespace,
            node.start_position().row as u32 + 1,
        ));
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, source, out);
    }
}

/// The namespace named by a `using` directive.
///
/// Handles `global using`, `using static`, and aliases: `using J = Newtonsoft.Json;`
/// depends on `Newtonsoft.Json` just as plainly as the unaliased form.
fn using_namespace(raw: &str) -> Option<String> {
    let mut rest = raw.trim().trim_end_matches(';').trim();
    rest = rest.strip_prefix("global ").unwrap_or(rest).trim_start();
    rest = rest.strip_prefix("using")?.trim_start();
    rest = rest.strip_prefix("static ").unwrap_or(rest).trim_start();
    rest = rest.strip_prefix("unsafe ").unwrap_or(rest).trim_start();

    // `using Alias = Some.Namespace` — the right-hand side is the dependency.
    if let Some((_, target)) = rest.split_once('=') {
        rest = target.trim();
    }

    // `using var x = ...` inside a method body is a resource statement, not an
    // import, and never reaches here as a `using_directive`. Guard anyway.
    let name = rest.trim();
    (!name.is_empty() && !name.contains(' ') && !name.contains('(')).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use camino::Utf8PathBuf;
    use tropism_core::model::Project;

    fn usings(source: &str) -> Vec<String> {
        extract_csharp_usings(Utf8Path::new("Foo.cs"), source)
            .unwrap()
            .into_iter()
            .map(|import| import.raw)
            .collect()
    }

    // --- csproj -----------------------------------------------------------

    const CSPROJ: &str = r#"<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net8.0</TargetFramework>
    <RootNamespace>Shop.Api</RootNamespace>
  </PropertyGroup>
  <ItemGroup>
    <PackageReference Include="Serilog" Version="4.0.0" />
    <PackageReference Include="StyleCop.Analyzers" Version="1.1.118" PrivateAssets="all" />
    <ProjectReference Include="..\Shop.Domain\Shop.Domain.csproj" />
  </ItemGroup>
</Project>"#;

    #[test]
    fn parses_package_and_project_references() {
        let manifest = parse_csproj(Utf8Path::new("Shop.Api/Shop.Api.csproj"), CSPROJ).unwrap();
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Serilog"), "got {names:?}");
        assert!(
            names.contains(&"Shop.Domain"),
            "project references are edges too"
        );
    }

    #[test]
    fn uses_the_root_namespace_as_the_package_identity() {
        let manifest = parse_csproj(Utf8Path::new("Shop.Api/Shop.Api.csproj"), CSPROJ).unwrap();
        assert_eq!(manifest.package_name.as_deref(), Some("Shop.Api"));
    }

    #[test]
    fn falls_back_to_the_project_filename() {
        let manifest = parse_csproj(
            Utf8Path::new("Shop.Web/Shop.Web.csproj"),
            "<Project><ItemGroup /></Project>",
        )
        .unwrap();
        assert_eq!(manifest.package_name.as_deref(), Some("Shop.Web"));
    }

    /// An analyzer package is never referenced from code, so it must not look like
    /// an unused dependency.
    #[test]
    fn private_assets_marks_a_build_only_package() {
        let manifest = parse_csproj(Utf8Path::new("A/A.csproj"), CSPROJ).unwrap();
        let analyzer = manifest
            .deps
            .iter()
            .find(|d| d.name == "StyleCop.Analyzers")
            .unwrap();
        assert_eq!(analyzer.kind, DepKind::Tooling);
        assert!(!analyzer.kind.expects_direct_import());
    }

    #[test]
    fn resolves_a_windows_style_project_reference_path() {
        assert_eq!(
            project_name_of(r"..\Shop.Domain\Shop.Domain.csproj"),
            "Shop.Domain"
        );
        assert_eq!(
            project_name_of("../Shop.Domain/Shop.Domain.csproj"),
            "Shop.Domain"
        );
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_silent_empty_result() {
        assert!(parse_csproj(Utf8Path::new("A/A.csproj"), "<Project><unclosed>").is_err());
    }

    // --- packages.lock.json -----------------------------------------------

    const LOCK: &str = r#"{
      "version": 1,
      "dependencies": {
        "net8.0": {
          "Serilog": { "type": "Direct", "resolved": "4.0.0" },
          "Serilog.Sinks.Console": {
            "type": "Transitive", "resolved": "6.0.0",
            "dependencies": { "Serilog": "4.0.0" }
          }
        }
      }
    }"#;

    #[test]
    fn builds_a_resolved_tree_when_a_lockfile_exists() {
        let resolved = parse_packages_lock(LOCK).unwrap().unwrap();
        assert_eq!(resolved.len(), 2);
        let sink = resolved
            .iter()
            .find(|d| d.name == "Serilog.Sinks.Console")
            .unwrap();
        assert_eq!(sink.dependencies, vec!["Serilog 4.0.0"]);
    }

    #[test]
    fn a_lockfile_without_dependencies_yields_no_tree() {
        assert!(parse_packages_lock(r#"{"version": 1}"#).unwrap().is_none());
    }

    // --- using directives -------------------------------------------------

    #[test]
    fn extracts_plain_usings() {
        assert_eq!(
            usings("using System;\nusing Shop.Domain.Orders;\n"),
            vec!["System", "Shop.Domain.Orders"]
        );
    }

    #[test]
    fn extracts_global_static_and_aliased_usings() {
        let source = concat!(
            "global using System.Linq;\n",
            "using static System.Math;\n",
            "using J = Newtonsoft.Json;\n",
        );
        assert_eq!(
            usings(source),
            vec!["System.Linq", "System.Math", "Newtonsoft.Json"]
        );
    }

    /// A `using` statement inside a method body is resource disposal, not an
    /// import, and must not be extracted.
    #[test]
    fn ignores_using_statements_in_method_bodies() {
        let source = concat!(
            "using System.IO;\n",
            "class C {\n",
            "  void M() {\n",
            "    using var stream = File.OpenRead(\"x\");\n",
            "  }\n",
            "}\n",
        );
        assert_eq!(usings(source), vec!["System.IO"]);
    }

    #[test]
    fn ignores_using_like_text_in_strings_and_comments() {
        let source = concat!(
            "using System;\n",
            "// using Fake.Commented;\n",
            "/* using Fake.Blocked; */\n",
            "class C { string s = \"using Fake.Quoted;\"; }\n",
        );
        assert_eq!(usings(source), vec!["System"]);
    }

    // --- namespaces -------------------------------------------------------

    #[test]
    fn reads_block_and_file_scoped_namespaces() {
        assert_eq!(
            namespace_of("namespace Shop.Api;\n").as_deref(),
            Some("Shop.Api")
        );
        assert_eq!(
            namespace_of("namespace Shop.Api\n{\n}\n").as_deref(),
            Some("Shop.Api")
        );
        assert_eq!(
            namespace_of("namespace Shop.Api {\n}\n").as_deref(),
            Some("Shop.Api")
        );
    }

    #[test]
    fn a_module_is_the_declared_namespace_not_the_path() {
        let id = CSharpProvider.module_id_for_file(
            Utf8Path::new("Shop.Api/Controllers/OrderController.cs"),
            "namespace Shop.Api.Handlers;\n",
            "Shop.Api/Controllers",
        );
        assert_eq!(id, ModuleId::module("Shop.Api.Handlers"));
    }

    #[test]
    fn a_test_project_is_a_separate_target() {
        let id = CSharpProvider.module_id_for_file(
            Utf8Path::new("Shop.Domain.Tests/OrderTests.cs"),
            "namespace Shop.Domain.Tests;\n",
            "Shop.Domain.Tests",
        );
        assert_eq!(id, ModuleId::external_test("Shop.Domain.Tests"));
    }

    // --- resolution -------------------------------------------------------

    fn resolve(
        namespace: &str,
        root: &str,
        modules: &[&str],
        declared: &[&str],
        siblings: &[&str],
    ) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::new(),
            language: Language::CSharp,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: "1.0.0".to_owned(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("A.csproj", Some(1)),
            })
            .collect();
        let known: BTreeSet<String> = modules.iter().map(|m| (*m).to_owned()).collect();
        let sibling_names: Vec<String> = siblings.iter().map(|s| (*s).to_owned()).collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: Some(root),
            declared: &deps,
            sibling_packages: &sibling_names,
            known_modules: &known,
            source_files: &[],
        };
        CSharpProvider.resolve_import(
            &Import::statement(namespace, 1),
            Utf8Path::new("A/Foo.cs"),
            &ctx,
        )
    }

    #[test]
    fn framework_namespaces_need_no_reference() {
        for namespace in ["System", "System.Linq", "System.Collections.Generic"] {
            assert_eq!(
                resolve(namespace, "App", &[], &[], &[]),
                ImportTarget::Stdlib
            );
        }
    }

    /// `SystemTextJson` is not in the `System` namespace.
    #[test]
    fn a_prefix_only_matches_on_a_dot_boundary() {
        assert_ne!(
            resolve("SystemX.Thing", "App", &[], &[], &[]),
            ImportTarget::Stdlib
        );
    }

    #[test]
    fn a_declared_namespace_resolves_internally() {
        assert_eq!(
            resolve(
                "Shop.Api.Handlers",
                "Shop.Api",
                &["Shop.Api.Handlers"],
                &[],
                &[]
            ),
            ImportTarget::Internal("Shop.Api.Handlers".to_owned())
        );
    }

    /// `using Shop.Api.Handlers.Impl` may name a type inside the `Shop.Api.Handlers`
    /// namespace, so the longest declared prefix wins.
    #[test]
    fn resolves_to_the_longest_declared_namespace() {
        assert_eq!(
            resolve(
                "Shop.Api.Handlers.Impl",
                "Shop.Api",
                &["Shop.Api", "Shop.Api.Handlers"],
                &[],
                &[]
            ),
            ImportTarget::Internal("Shop.Api.Handlers".to_owned())
        );
    }

    #[test]
    fn a_package_reference_resolves_externally() {
        assert_eq!(
            resolve("Serilog.Events", "App", &[], &["Serilog"], &[]),
            ImportTarget::External("Serilog".to_owned())
        );
    }

    /// NuGet ids are case-insensitive, so `using AWSSDK...` must match `AWSSDK`.
    #[test]
    fn package_matching_is_case_insensitive() {
        assert_eq!(
            resolve(
                "newtonsoft.json.Linq",
                "App",
                &[],
                &["Newtonsoft.Json"],
                &[]
            ),
            ImportTarget::External("Newtonsoft.Json".to_owned())
        );
    }

    #[test]
    fn a_sibling_project_resolves_to_that_project() {
        assert_eq!(
            resolve("Shop.Domain.Orders", "Shop.Api", &[], &[], &["Shop.Domain"]),
            ImportTarget::External("Shop.Domain".to_owned())
        );
    }

    /// The residue where the package name is not a prefix of the namespace.
    #[test]
    fn the_exception_table_covers_namespaces_that_do_not_match_their_package() {
        assert_eq!(
            resolve("Xunit", "App", &[], &[], &[]),
            ImportTarget::External("xunit".to_owned())
        );
        assert_eq!(
            resolve("NUnit.Framework", "App", &[], &[], &[]),
            ImportTarget::External("NUnit".to_owned())
        );
    }

    /// A namespace matching nothing is more likely an unseen project reference than
    /// an undeclared package, so it must not become a missing-dependency finding.
    #[test]
    fn an_unrecognised_namespace_is_unresolved() {
        assert!(matches!(
            resolve("Some.Other.Company.Lib", "App", &[], &[], &[]),
            ImportTarget::Unresolved { .. }
        ));
    }
}
