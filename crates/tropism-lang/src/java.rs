//! Java provider.
//!
//! Java has the widest gap in this codebase between what a dependency is *called*
//! and what it is *imported as*. A Maven coordinate is `groupId:artifactId`; an
//! import is a package. `com.google.guava:guava` is imported as `com.google.common`,
//! and `com.fasterxml.jackson.core:jackson-databind` as
//! `com.fasterxml.jackson.databind`. The convention that a package starts with its
//! groupId holds often enough to be the primary rule and not often enough to be the
//! only one, so a curated table covers the well-known residue and anything else
//! stays [`ImportTarget::Unresolved`].
//!
//! Three things follow from Java's build tools rather than the language:
//!
//! * **Maven has no lockfile.** None. The resolved tree exists only inside a
//!   `mvn` invocation, which this tool will not run.
//! * **`gradle.lockfile` is opt-in and edge-free.** It records the version each
//!   configuration selected, one coordinate per line, with no dependency
//!   relationships — so it can no more answer a diamond question than `go.sum` can.
//! * **The compile classpath is transitive.** Code can import a class from a
//!   dependency of a dependency and compile cleanly, which is why an unmatched
//!   import here is more often a transitive reach than a missing declaration.
//!
//! `build.gradle` is a program, like Ruby's `Gemfile`: the declarative subset is
//! read and dynamic constructs contribute nothing.

use std::collections::BTreeSet;

use camino::Utf8Path;
use tropism_core::graph::ModuleId;
use tropism_core::model::{DeclaredDep, DepKind, Language, Manifest, Provenance, ResolvedDep};
use tropism_core::provider::{Import, ImportTarget, LanguageProvider, ProjectContext, VersionOps};
use tropism_core::workspace::WorkspaceDecl;

pub struct JavaProvider;

/// Package roots supplied by the platform.
///
/// `javax` is the awkward one. Some of it is in the JDK, some shipped as Java EE
/// artifacts, and the rest moved to `jakarta`. Treating it as platform can hide a
/// genuinely missing declaration; the alternative reports a missing dependency on
/// `javax.annotation` for every project that predates the split, which is worse.
/// The same trade as `System.*` in the C# provider.
const PLATFORM_ROOTS: &[&str] = &[
    "java", "javax", "jdk", "sun", "com.sun", "org.w3c", "org.xml",
];

/// Packages whose coordinate does not begin with the groupId.
///
/// The residue after longest-groupId matching, which handles the ordinary case.
/// Kept small and curated: a wrong entry becomes a confident false finding, so
/// anything unfamiliar is left unresolved instead.
const PACKAGE_TO_COORDINATE: &[(&str, &str)] = &[
    ("com.google.common", "com.google.guava:guava"),
    ("com.google.gson", "com.google.code.gson:gson"),
    ("okhttp3", "com.squareup.okhttp3:okhttp"),
    ("okio", "com.squareup.okio:okio"),
    ("retrofit2", "com.squareup.retrofit2:retrofit"),
    ("lombok", "org.projectlombok:lombok"),
    ("org.junit.jupiter", "org.junit.jupiter:junit-jupiter"),
    ("org.junit", "junit:junit"),
    (
        "org.apache.commons.lang3",
        "org.apache.commons:commons-lang3",
    ),
    ("org.apache.commons.io", "commons-io:commons-io"),
    (
        "org.apache.commons.collections4",
        "org.apache.commons:commons-collections4",
    ),
    ("org.jetbrains.annotations", "org.jetbrains:annotations"),
    ("org.yaml.snakeyaml", "org.yaml:snakeyaml"),
    ("io.reactivex.rxjava3", "io.reactivex.rxjava3:rxjava"),
];

/// Gradle configurations that declare a dependency, and the kind each implies.
const GRADLE_CONFIGURATIONS: &[(&str, DepKind)] = &[
    ("implementation", DepKind::Runtime),
    ("api", DepKind::Runtime),
    ("compileOnly", DepKind::Build),
    ("compileOnlyApi", DepKind::Build),
    // On the classpath at runtime and never compiled against — a JDBC driver, a
    // logging backend. The same shape as a Roslyn analyzer or an npm CLI.
    ("runtimeOnly", DepKind::Tooling),
    ("annotationProcessor", DepKind::Tooling),
    ("kapt", DepKind::Tooling),
    ("testImplementation", DepKind::Dev),
    ("testCompileOnly", DepKind::Dev),
    ("testRuntimeOnly", DepKind::Tooling),
];

struct MavenVersionOps;

impl VersionOps for MavenVersionOps {
    /// Maven orders a version by dot- and dash-separated segments, comparing
    /// numerics numerically. Only the numeric prefix is needed here; `None` on a
    /// qualifier keeps a wrong ordering out of a finding.
    fn compare(&self, a: &str, b: &str) -> Option<std::cmp::Ordering> {
        let parse = |version: &str| -> Option<Vec<u64>> {
            version
                .trim()
                .split(['.', '-'])
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

impl LanguageProvider for JavaProvider {
    fn language(&self) -> Language {
        Language::Java
    }

    fn manifest_names(&self) -> &'static [&'static str] {
        &["pom.xml", "build.gradle", "build.gradle.kts"]
    }

    /// Opt-in (`dependencyLocking`) and usually absent. Parsed only to be reported
    /// as insufficient — see [`Self::resolved_tree_note`].
    fn lockfile_names(&self) -> &'static [&'static str] {
        &["gradle.lockfile"]
    }

    /// Gradle states its multi-project build in `settings.gradle`, which is not a
    /// manifest — `build.gradle` is.
    fn workspace_files(&self) -> &'static [&'static str] {
        &["settings.gradle", "settings.gradle.kts"]
    }

    /// Maven's `<modules>` and Gradle's `include` are the two multi-project
    /// declarations, and both are read for the same reason: a reactor module
    /// importing its sibling's classes is a workspace fact, not an undeclared
    /// dependency.
    fn workspace_members(&self, path: &Utf8Path, text: &str) -> Option<WorkspaceDecl> {
        match path.file_name() {
            Some("pom.xml") => parse_maven_modules(text),
            Some("settings.gradle" | "settings.gradle.kts") => {
                Some(WorkspaceDecl::members(parse_gradle_includes(text)))
            }
            _ => None,
        }
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["java"]
    }

    fn parse_manifest(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
        if path.file_name() == Some("pom.xml") {
            parse_pom(path, text)
        } else {
            Ok(parse_gradle(path, text))
        }
    }

    /// Deliberately no resolved tree, for the same reason as Go's `go.sum`.
    ///
    /// `gradle.lockfile` is a flat list of `group:artifact:version=configurations`
    /// lines. The versions are real, but there are no edges at all, so a diamond
    /// question has nothing to traverse. Returning the flat list would let
    /// `diamond-dep` report a confident `0 findings` about a graph it never had.
    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        _text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        Ok(None)
    }

    fn resolved_tree_note(&self) -> Option<&'static str> {
        Some(
            "gradle.lockfile records the version each configuration selected, one \
             coordinate per line, and carries no edges; Maven has no lockfile at all, \
             so a resolved tree needs the build tool",
        )
    }

    fn extract_imports(&self, path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
        extract_java_imports(path, text)
    }

    /// A Java module is the package the file declares.
    ///
    /// The directory is required to match it in a conventional build, but it is the
    /// `package` statement that imports actually name, and a source root
    /// (`src/main/java`) sits in the path without being part of it. The C# provider
    /// reached the same conclusion for the same reason.
    fn module_id_for_file(&self, path: &Utf8Path, text: &str, default_id: &str) -> ModuleId {
        let name = package_declaration(text).unwrap_or_else(|| default_id.to_owned());

        // `src/test/java` is compiled separately and exists to depend on the code
        // under test; that is never a cycle.
        let is_test =
            path.as_str().contains("/src/test/") || path.as_str().starts_with("src/test/");
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
        let package = import.raw.as_str();

        if is_platform(package) {
            return ImportTarget::Stdlib;
        }

        // 1. A package this project declares. Longest prefix wins, since
        //    `com.example.api.internal` may belong to the `com.example.api` module.
        if let Some(module) = longest_prefix_in(package, ctx.known_modules) {
            return ImportTarget::Internal(module);
        }

        // 2. A declared coordinate whose groupId prefixes the package. This is the
        //    convention, and it covers most of the ecosystem.
        let by_group = ctx
            .declared
            .iter()
            .map(|dep| dep.name.as_str())
            .chain(ctx.sibling_packages.iter().map(String::as_str))
            .filter(|coordinate| {
                group_id(coordinate).is_some_and(|group| is_dotted_prefix(package, group))
            })
            .max_by_key(|coordinate| group_id(coordinate).map_or(0, str::len));
        if let Some(coordinate) = by_group {
            return ImportTarget::External(coordinate.to_owned());
        }

        // 3. The curated residue, where the package and the groupId disagree.
        if let Some((_, coordinate)) = PACKAGE_TO_COORDINATE
            .iter()
            .filter(|(prefix, _)| is_dotted_prefix(package, prefix))
            .max_by_key(|(prefix, _)| prefix.len())
        {
            return ImportTarget::External((*coordinate).to_owned());
        }

        // Maven puts a dependency's own dependencies on the compile classpath, so
        // code can import a transitive artifact and compile cleanly. An unmatched
        // package is therefore more likely a transitive reach than an undeclared
        // one, and naming a coordinate for it would be a guess with no artifactId
        // in it. Staying unresolved caps hygiene confidence instead.
        ImportTarget::Unresolved {
            reason: format!("`{package}` matches no declared coordinate or known package"),
        }
    }

    /// An import names an absolute package, so the module inside the target project
    /// is the longest package that project declares.
    fn resolve_cross_project(
        &self,
        import: &Import,
        target: &ProjectContext<'_>,
    ) -> Option<String> {
        longest_prefix_in(&import.raw, target.known_modules)
    }

    fn is_stdlib(&self, module: &str) -> bool {
        is_platform(module)
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &MavenVersionOps
    }
}

fn is_platform(package: &str) -> bool {
    PLATFORM_ROOTS
        .iter()
        .any(|root| is_dotted_prefix(package, root))
}

/// The groupId half of a `groupId:artifactId` coordinate.
fn group_id(coordinate: &str) -> Option<&str> {
    coordinate.split_once(':').map(|(group, _)| group)
}

/// Whether `prefix` is a dotted-segment prefix of `package`. Guards against
/// `com.exampleX` matching `com.example`.
fn is_dotted_prefix(package: &str, prefix: &str) -> bool {
    package == prefix
        || (package.len() > prefix.len()
            && package.starts_with(prefix)
            && package.as_bytes()[prefix.len()] == b'.')
}

/// The longest `module` in `modules` that is a dotted prefix of `name`.
///
/// D39. Equivalent to filtering every module by [`is_dotted_prefix`] and taking the
/// longest, but probing the input's own prefixes from longest to shortest instead —
/// O(segments x log n) rather than O(modules), per import. The scan was quadratic
/// over a project and only visible above about a thousand files.
fn longest_prefix_in(name: &str, modules: &BTreeSet<String>) -> Option<String> {
    let mut candidate = name;
    loop {
        // "." is a project's root package and is a prefix of nothing.
        if candidate != "." && modules.contains(candidate) {
            return Some(candidate.to_owned());
        }
        candidate = &candidate[..candidate.rfind('.')?];
    }
}

/// The `package` statement, which is the first non-comment statement in the file.
fn package_declaration(text: &str) -> Option<String> {
    let mut in_block_comment = false;
    for raw in text.lines() {
        let line = raw.trim();
        if in_block_comment {
            match line.find("*/") {
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
        if let Some(rest) = line.strip_prefix("package ") {
            return Some(rest.trim().trim_end_matches(';').trim().to_owned());
        }
        // Anything else at the top level means the file has no package statement,
        // so it is in the unnamed package.
        if !line.starts_with('@') && !line.starts_with('*') {
            return None;
        }
    }
    None
}

// --- pom.xml -----------------------------------------------------------------

/// Parses a `pom.xml`.
///
/// One subtlety decides whether this is useful or a false-positive generator:
/// `<dependencyManagement>` declares *versions* for dependencies a module may
/// later use, exactly like `Directory.Packages.props` in .NET or Cargo's
/// `[workspace.dependencies]`. Its entries are not dependencies of the module that
/// carries them, and counting them as such reports every one unused.
/// `<modules><module>a</module></modules>` from an aggregator POM.
///
/// Only the top-level `<project><modules>` block counts: `<modules>` can also
/// appear inside a `<profile>`, where whether it applies depends on activation
/// tropism cannot evaluate without running Maven.
fn parse_maven_modules(text: &str) -> Option<WorkspaceDecl> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(text);
    let mut stack: Vec<String> = Vec::new();
    let mut buffer = String::new();
    let mut members = Vec::new();

    loop {
        match reader.read_event() {
            Err(_) | Ok(Event::Eof) => break,
            Ok(Event::Start(element)) => {
                stack.push(String::from_utf8_lossy(element.local_name().as_ref()).into_owned());
                buffer.clear();
            }
            Ok(Event::Text(event)) => buffer.push_str(&event.decode().unwrap_or_default()),
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                let value = buffer.trim().to_owned();
                // The chain must be exactly `project > modules > module`. Anything
                // deeper is a `<profile>`, whose activation cannot be evaluated
                // without running Maven.
                let top_level = stack.len() >= 3
                    && stack[stack.len() - 2] == "modules"
                    && stack[stack.len() - 3] == "project";
                if name == "module" && !value.is_empty() && top_level {
                    members.push(value);
                }
                stack.pop();
                buffer.clear();
            }
            Ok(_) => buffer.clear(),
        }
    }

    (!members.is_empty()).then(|| WorkspaceDecl::members(members))
}

/// `include ':a', ':b'` / `include("a:b")` from `settings.gradle[.kts]`.
///
/// A Gradle project path is colon-separated (`:services:api`); the directory it
/// maps to is the same path with separators swapped, unless a `projectDir` is set —
/// which is dynamic, so a remapped project simply contributes its default location
/// and is corrected by the language fallback if that is wrong.
fn parse_gradle_includes(text: &str) -> Vec<String> {
    let mut members = Vec::new();
    for raw_line in text.lines() {
        let line = strip_line_comment(raw_line).trim();
        if !starts_with_word(line, "include") {
            continue;
        }
        let rest = line.trim_start_matches("include").trim();
        // Both `include ':a', ':b'` and `include(":a", ":b")` are one list of
        // quoted literals once the punctuation is ignored.
        for chunk in rest.split(',') {
            if let Some(literal) = quoted_literal(chunk.trim().trim_start_matches('(')) {
                let path = literal.trim_start_matches(':').replace(':', "/");
                if !path.is_empty() {
                    members.push(path);
                }
            }
        }
    }
    members
}

fn parse_pom(path: &Utf8Path, text: &str) -> anyhow::Result<Manifest> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(text);
    reader.config_mut().check_end_names = true;

    let mut deps: Vec<DeclaredDep> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    let mut buffer = String::new();

    // The dependency currently being read, and the project's own coordinate.
    let mut current: Option<PartialDep> = None;
    let mut group: Option<String> = None;
    let mut artifact: Option<String> = None;

    loop {
        match reader.read_event() {
            Err(error) => anyhow::bail!("{path}: {error}"),
            Ok(Event::Eof) => {
                if !stack.is_empty() {
                    anyhow::bail!("{path}: {} unclosed element(s)", stack.len());
                }
                break;
            }
            Ok(Event::Start(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                if name == "dependency" && !in_managed(&stack) {
                    current = Some(PartialDep::at(reader.buffer_position()));
                }
                stack.push(name);
                buffer.clear();
            }
            Ok(Event::Text(event)) => buffer.push_str(&event.decode().unwrap_or_default()),
            Ok(Event::End(element)) => {
                let name = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                let value = buffer.trim().to_owned();
                stack.pop();
                buffer.clear();

                if let Some(dep) = current.as_mut() {
                    match name.as_str() {
                        "groupId" => dep.group = value,
                        "artifactId" => dep.artifact = value,
                        "version" => dep.version = value,
                        "scope" => dep.scope = value,
                        "optional" => dep.optional = value.eq_ignore_ascii_case("true"),
                        "dependency" => {
                            if let Some(finished) = current.take() {
                                deps.push(finished.into_declared(path, text));
                            }
                        }
                        _ => {}
                    }
                    continue;
                }

                // The project's own coordinate: `<groupId>` and `<artifactId>`
                // directly under `<project>`. `<parent>` carries the same element
                // names one level deeper and must not overwrite them.
                if stack.as_slice() == ["project"] {
                    match name.as_str() {
                        "groupId" => group = Some(value),
                        "artifactId" => artifact = Some(value),
                        _ => {}
                    }
                }
            }
            Ok(_) => {}
        }
    }

    deps.sort_by(|a, b| (&a.name, a.kind).cmp(&(&b.name, b.kind)));
    deps.dedup_by(|a, b| a.name == b.name);

    // A module inheriting its groupId from `<parent>` still needs an identity, so
    // the artifactId alone is better than nothing.
    let package_name = match (group, artifact) {
        (Some(group), Some(artifact)) => Some(format!("{group}:{artifact}")),
        (None, Some(artifact)) => Some(artifact),
        _ => None,
    };

    Ok(Manifest { deps, package_name })
}

/// Whether the current element sits inside `<dependencyManagement>`.
fn in_managed(stack: &[String]) -> bool {
    stack.iter().any(|name| name == "dependencyManagement")
}

#[derive(Default)]
struct PartialDep {
    group: String,
    artifact: String,
    version: String,
    scope: String,
    optional: bool,
    position: u64,
}

impl PartialDep {
    fn at(position: u64) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }

    fn into_declared(self, path: &Utf8Path, text: &str) -> DeclaredDep {
        let kind = if self.optional {
            DepKind::Optional
        } else {
            match self.scope.as_str() {
                "test" => DepKind::Dev,
                "provided" => DepKind::Build,
                // On the classpath but never compiled against: a JDBC driver, a
                // logging backend. Expecting an import would report every one.
                "runtime" | "system" => DepKind::Tooling,
                _ => DepKind::Runtime,
            }
        };

        DeclaredDep {
            name: format!("{}:{}", self.group, self.artifact),
            requirement: self.version,
            kind,
            declared_at: Provenance::new(path, line_at(text, self.position)),
        }
    }
}

/// quick-xml reports a byte offset; findings cite lines.
fn line_at(text: &str, position: u64) -> Option<u32> {
    let offset = usize::try_from(position).ok()?.min(text.len());
    Some(text[..offset].lines().count().max(1) as u32)
}

// --- build.gradle ------------------------------------------------------------

/// Parses the declarative subset of `build.gradle` or `build.gradle.kts`.
///
/// A Gradle build script is a program in Groovy or Kotlin, and the constraint
/// forbids running it. What is read is the shape that covers most real files: a
/// configuration name followed by a quoted `group:artifact:version` coordinate.
///
/// Two things are deliberately *not* guessed. A coordinate assembled from
/// variables or interpolation (`implementation "org.x:y:$version"`) names a
/// version tropism cannot know, so only the version is dropped — the coordinate is
/// still real. A version-catalog reference (`implementation libs.guava`) names
/// nothing at all without reading `gradle/libs.versions.toml` in a second file,
/// and is skipped rather than invented.
fn parse_gradle(path: &Utf8Path, text: &str) -> Manifest {
    let mut deps: Vec<DeclaredDep> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line = strip_line_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        let Some((configuration, kind)) = GRADLE_CONFIGURATIONS
            .iter()
            .find(|(name, _)| starts_with_word(line, name))
            .copied()
        else {
            continue;
        };

        let rest = line[configuration.len()..].trim_start();
        let Some(literal) = quoted_literal(rest) else {
            // `implementation libs.guava` or `implementation project(":core")` —
            // neither names a coordinate that can be read from this file alone.
            continue;
        };

        let mut parts = literal.splitn(3, ':');
        let (Some(group), Some(artifact)) = (parts.next(), parts.next()) else {
            continue;
        };
        if group.is_empty() || artifact.is_empty() {
            continue;
        }
        let version = parts.next().unwrap_or_default();

        deps.push(DeclaredDep {
            name: format!("{group}:{artifact}"),
            // An interpolated version is unknowable without evaluating the script.
            requirement: if version.contains('$') {
                String::new()
            } else {
                version.to_owned()
            },
            kind,
            declared_at: Provenance::new(path, Some(index as u32 + 1)),
        });
    }

    deps.sort_by(|a, b| (&a.name, a.kind).cmp(&(&b.name, b.kind)));
    deps.dedup_by(|a, b| a.name == b.name);

    Manifest {
        deps,
        // A Gradle script's own coordinate lives in `group`/`version` properties or
        // a `publishing` block, neither of which is reliable enough to claim.
        package_name: None,
    }
}

fn strip_line_comment(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

/// Whether `line` begins with `word` followed by a non-identifier character, so
/// `api` does not match `apiElements`.
fn starts_with_word(line: &str, word: &str) -> bool {
    line.strip_prefix(word).is_some_and(|rest| {
        rest.chars()
            .next()
            .is_none_or(|ch| !ch.is_alphanumeric() && ch != '_')
    })
}

/// The first single- or double-quoted string on the line.
fn quoted_literal(rest: &str) -> Option<&str> {
    let bytes = rest.as_bytes();
    let open = bytes
        .iter()
        .position(|byte| *byte == b'"' || *byte == b'\'')?;
    let quote = bytes[open];
    let close = open + 1 + rest[open + 1..].find(quote as char)?;
    Some(&rest[open + 1..close])
}

// --- import extraction -------------------------------------------------------

/// Extracts every `import` from one Java file.
///
/// `import java.util.*;` and `import static org.junit.Assert.assertEquals;` both
/// name a package once the trailing wildcard or member is removed, which is the
/// only granularity resolution needs.
fn extract_java_imports(path: &Utf8Path, text: &str) -> anyhow::Result<Vec<Import>> {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .map_err(|error| anyhow::anyhow!("loading the Java grammar failed: {error}"))?;
    let tree = parser
        .parse(text, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter could not parse `{path}`"))?;

    let mut imports = Vec::new();
    collect_imports(tree.root_node(), text.as_bytes(), &mut imports);
    Ok(imports)
}

fn collect_imports(node: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<Import>) {
    if node.kind() == "import_declaration" {
        let is_static = node
            .utf8_text(source)
            .is_ok_and(|text| text.trim_start().starts_with("import static"));

        let mut cursor = node.walk();
        let name = node
            .named_children(&mut cursor)
            .find(|child| matches!(child.kind(), "scoped_identifier" | "identifier"))
            .and_then(|child| child.utf8_text(source).ok());

        if let Some(name) = name {
            // `import static a.b.C.method` names the member; the package is what
            // resolution needs, and the class is one segment up from the member.
            let package = if is_static {
                trim_last_segment(name)
            } else {
                name
            };
            out.push(Import::statement(
                package.to_owned(),
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

fn trim_last_segment(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(head, _)| head)
}

#[cfg(test)]
mod tests {

    /// D39 replaced a scan of every module with prefix probes. The contract is
    /// unchanged: the *longest* dotted prefix wins, and a partial segment is not a
    /// prefix at all.
    #[test]
    fn the_longest_dotted_prefix_wins_and_partial_segments_do_not_match() {
        let modules: BTreeSet<String> = ["com", "com.shop", "com.shopping"]
            .into_iter()
            .map(str::to_owned)
            .collect();

        assert_eq!(
            longest_prefix_in("com.shop.orders.Item", &modules),
            Some("com.shop".to_owned())
        );
        assert_eq!(
            longest_prefix_in("com.other", &modules),
            Some("com".to_owned())
        );
        // `com.shopp` must not match `com.shop` — segment boundaries matter.
        assert_eq!(
            longest_prefix_in("com.shopp", &modules),
            Some("com".to_owned())
        );
        assert_eq!(longest_prefix_in("org.example", &modules), None);
    }

    #[test]
    fn an_aggregator_pom_declares_its_modules() {
        let decl = JavaProvider
            .workspace_members(
                Utf8Path::new("pom.xml"),
                r#"<project><modules><module>api</module><module>core</module></modules></project>"#,
            )
            .expect("a <modules> block declares a workspace");
        assert_eq!(decl.members, vec!["api", "core"]);
    }

    /// A `<modules>` inside a `<profile>` applies only when the profile activates,
    /// which cannot be evaluated without running Maven. Contributing nothing is the
    /// same rule the Gradle and Gemfile parsers follow for dynamic constructs.
    #[test]
    fn modules_inside_a_profile_are_not_read() {
        assert!(
            JavaProvider
                .workspace_members(
                    Utf8Path::new("pom.xml"),
                    r#"<project><profiles><profile><modules><module>only-sometimes</module></modules></profile></profiles></project>"#,
                )
                .is_none()
        );
    }

    #[test]
    fn gradle_include_maps_a_project_path_to_a_directory() {
        let decl = JavaProvider
            .workspace_members(
                Utf8Path::new("settings.gradle"),
                "rootProject.name = 'app'\ninclude ':api', ':services:worker'\ninclude(\"lib\")\n",
            )
            .unwrap();
        assert_eq!(decl.members, vec!["api", "services/worker", "lib"]);
    }
    use super::*;
    use camino::Utf8PathBuf;
    use std::collections::BTreeSet;
    use tropism_core::model::Project;

    fn packages(source: &str) -> Vec<String> {
        extract_java_imports(Utf8Path::new("A.java"), source)
            .unwrap()
            .into_iter()
            .map(|import| import.raw)
            .collect()
    }

    // --- pom.xml ----------------------------------------------------------

    fn pom(body: &str) -> Manifest {
        parse_pom(Utf8Path::new("pom.xml"), body).unwrap()
    }

    #[test]
    fn parses_coordinates_and_the_projects_own_identity() {
        let manifest = pom(concat!(
            "<project>\n",
            "  <groupId>com.example</groupId>\n",
            "  <artifactId>shop</artifactId>\n",
            "  <dependencies>\n",
            "    <dependency>\n",
            "      <groupId>org.slf4j</groupId>\n",
            "      <artifactId>slf4j-api</artifactId>\n",
            "      <version>2.0.9</version>\n",
            "    </dependency>\n",
            "  </dependencies>\n",
            "</project>\n",
        ));
        assert_eq!(manifest.package_name.as_deref(), Some("com.example:shop"));
        assert_eq!(manifest.deps.len(), 1);
        assert_eq!(manifest.deps[0].name, "org.slf4j:slf4j-api");
        assert_eq!(manifest.deps[0].requirement, "2.0.9");
    }

    #[test]
    fn maps_scopes_to_dependency_kinds() {
        let manifest = pom(concat!(
            "<project><dependencies>\n",
            "<dependency><groupId>a</groupId><artifactId>t</artifactId><scope>test</scope></dependency>\n",
            "<dependency><groupId>a</groupId><artifactId>p</artifactId><scope>provided</scope></dependency>\n",
            "<dependency><groupId>a</groupId><artifactId>r</artifactId><scope>runtime</scope></dependency>\n",
            "</dependencies></project>\n",
        ));
        let kind = |artifact: &str| {
            manifest
                .deps
                .iter()
                .find(|d| d.name == format!("a:{artifact}"))
                .unwrap()
                .kind
        };
        assert_eq!(kind("t"), DepKind::Dev);
        assert_eq!(kind("p"), DepKind::Build);
        assert_eq!(
            kind("r"),
            DepKind::Tooling,
            "a runtime-scope artifact is on the classpath and never imported"
        );
    }

    /// `<dependencyManagement>` sets versions for dependencies a module *may* use.
    /// Counting its entries as declared reports every one of them unused.
    #[test]
    fn dependency_management_entries_are_not_dependencies() {
        let manifest = pom(concat!(
            "<project>\n",
            "  <dependencyManagement>\n",
            "    <dependencies>\n",
            "      <dependency><groupId>org.x</groupId><artifactId>y</artifactId><version>1.0</version></dependency>\n",
            "    </dependencies>\n",
            "  </dependencyManagement>\n",
            "</project>\n",
        ));
        assert!(manifest.deps.is_empty(), "got {:?}", manifest.deps);
    }

    /// `<parent>` carries `groupId` and `artifactId` too, one level deeper.
    #[test]
    fn a_parent_coordinate_is_not_the_projects_own() {
        let manifest = pom(concat!(
            "<project>\n",
            "  <parent><groupId>com.parent</groupId><artifactId>base</artifactId></parent>\n",
            "  <artifactId>child</artifactId>\n",
            "</project>\n",
        ));
        assert_eq!(manifest.package_name.as_deref(), Some("child"));
    }

    #[test]
    fn an_unbalanced_pom_is_an_error_not_an_empty_manifest() {
        assert!(parse_pom(Utf8Path::new("pom.xml"), "<project><dependencies>").is_err());
    }

    // --- build.gradle -----------------------------------------------------

    fn gradle(text: &str) -> Manifest {
        parse_gradle(Utf8Path::new("build.gradle"), text)
    }

    #[test]
    fn parses_gradle_configurations() {
        let manifest = gradle(concat!(
            "dependencies {\n",
            "    implementation 'org.slf4j:slf4j-api:2.0.9'\n",
            "    testImplementation(\"org.junit.jupiter:junit-jupiter:5.10.2\")\n",
            "    runtimeOnly 'org.postgresql:postgresql:42.7.3'\n",
            "}\n",
        ));
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "org.junit.jupiter:junit-jupiter",
                "org.postgresql:postgresql",
                "org.slf4j:slf4j-api"
            ]
        );
        let kind = |name: &str| manifest.deps.iter().find(|d| d.name == name).unwrap().kind;
        assert_eq!(kind("org.junit.jupiter:junit-jupiter"), DepKind::Dev);
        assert_eq!(kind("org.postgresql:postgresql"), DepKind::Tooling);
    }

    /// An interpolated version is unknowable, but the coordinate around it is real
    /// and dropping the whole line would lose a genuine dependency.
    #[test]
    fn an_interpolated_version_keeps_the_coordinate_and_drops_the_version() {
        let manifest = gradle("implementation \"com.google.guava:guava:$guavaVersion\"\n");
        assert_eq!(manifest.deps[0].name, "com.google.guava:guava");
        assert_eq!(manifest.deps[0].requirement, "");
    }

    /// A version catalog reference names nothing without a second file.
    #[test]
    fn a_version_catalog_reference_is_skipped_rather_than_guessed() {
        let manifest = gradle("implementation libs.guava\nimplementation project(':core')\n");
        assert!(manifest.deps.is_empty());
    }

    #[test]
    fn a_commented_out_dependency_is_not_declared() {
        let manifest = gradle("// implementation 'org.x:y:1.0'\nimplementation 'org.a:b:1.0'\n");
        let names: Vec<&str> = manifest.deps.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["org.a:b"]);
    }

    /// `api` must not match `apiElements`.
    #[test]
    fn a_configuration_name_matches_only_on_a_word_boundary() {
        assert!(starts_with_word("api 'g:a:1'", "api"));
        assert!(!starts_with_word("apiElements 'g:a:1'", "api"));
    }

    // --- lockfile ---------------------------------------------------------

    #[test]
    fn a_gradle_lockfile_is_not_treated_as_a_resolved_tree() {
        let parsed = JavaProvider
            .parse_lockfile(
                Utf8Path::new("gradle.lockfile"),
                "com.google.guava:guava:32.1.3-jre=compileClasspath\n",
            )
            .unwrap();
        assert!(parsed.is_none());
        assert!(
            JavaProvider
                .resolved_tree_note()
                .unwrap()
                .contains("no edges")
        );
    }

    // --- import extraction ------------------------------------------------

    #[test]
    fn extracts_ordinary_wildcard_and_static_imports() {
        let source = concat!(
            "package com.example;\n",
            "import java.util.List;\n",
            "import java.util.*;\n",
            "import static org.junit.jupiter.api.Assertions.assertEquals;\n",
        );
        assert_eq!(
            packages(source),
            vec![
                "java.util.List",
                "java.util",
                "org.junit.jupiter.api.Assertions"
            ]
        );
    }

    /// The reason for a grammar instead of a regex.
    #[test]
    fn ignores_imports_in_strings_and_comments() {
        let source = concat!(
            "package a;\n",
            "import java.util.List;\n",
            "// import fake.commented.Thing;\n",
            "/* import fake.block.Thing; */\n",
            "class A { String s = \"import fake.quoted.Thing;\"; }\n",
        );
        assert_eq!(packages(source), vec!["java.util.List"]);
    }

    #[test]
    fn reports_accurate_line_numbers() {
        let imports = extract_java_imports(
            Utf8Path::new("A.java"),
            "package a;\n\nimport java.util.List;\n",
        )
        .unwrap();
        assert_eq!(imports[0].line, 3);
    }

    // --- module identity --------------------------------------------------

    #[test]
    fn a_module_is_the_declared_package() {
        let id = JavaProvider.module_id_for_file(
            Utf8Path::new("src/main/java/com/example/api/A.java"),
            "package com.example.api;\n",
            "src/main/java/com/example/api",
        );
        assert_eq!(id.name, "com.example.api");
    }

    #[test]
    fn the_package_statement_is_found_past_a_licence_header() {
        let text = "/*\n * Copyright 2026.\n */\n\npackage com.example.api;\n";
        assert_eq!(
            package_declaration(text).as_deref(),
            Some("com.example.api")
        );
    }

    #[test]
    fn test_sources_are_external_test_modules() {
        let id = JavaProvider.module_id_for_file(
            Utf8Path::new("api/src/test/java/com/example/api/ATest.java"),
            "package com.example.api;\n",
            "api/src/test/java/com/example/api",
        );
        assert!(id.kind.is_test());
    }

    // --- resolution -------------------------------------------------------

    fn resolve(raw: &str, declared: &[&str], modules: &[&str]) -> ImportTarget {
        let project = Project {
            root: Utf8PathBuf::from("api"),
            language: Language::Java,
            manifests: vec![],
            lockfile: None,
        };
        let deps: Vec<DeclaredDep> = declared
            .iter()
            .map(|name| DeclaredDep {
                name: (*name).to_owned(),
                requirement: String::new(),
                kind: DepKind::Runtime,
                declared_at: Provenance::new("pom.xml", Some(1)),
            })
            .collect();
        let known: BTreeSet<String> = modules.iter().map(|m| (*m).to_owned()).collect();
        let ctx = ProjectContext {
            project: &project,
            package_name: Some("com.example:shop"),
            declared: &deps,
            sibling_packages: &[],
            known_modules: &known,
            source_files: &[],
            local_modules: Default::default(),
            path_aliases: &[],
        };
        JavaProvider.resolve_import(
            &Import::statement(raw, 1),
            Utf8Path::new("api/src/main/java/A.java"),
            &ctx,
        )
    }

    #[test]
    fn resolves_platform_packages() {
        assert_eq!(resolve("java.util.List", &[], &[]), ImportTarget::Stdlib);
        assert_eq!(
            resolve("javax.sql.DataSource", &[], &[]),
            ImportTarget::Stdlib
        );
    }

    #[test]
    fn resolves_a_declared_coordinate_by_its_group_id() {
        assert_eq!(
            resolve("org.slf4j.Logger", &["org.slf4j:slf4j-api"], &[]),
            ImportTarget::External("org.slf4j:slf4j-api".to_owned())
        );
    }

    #[test]
    fn the_longest_matching_group_id_wins() {
        assert_eq!(
            resolve(
                "com.fasterxml.jackson.databind.ObjectMapper",
                &["com.fasterxml:base", "com.fasterxml.jackson:core"],
                &[]
            ),
            ImportTarget::External("com.fasterxml.jackson:core".to_owned())
        );
    }

    /// The import→package problem: guava's groupId is `com.google.guava` and its
    /// package is `com.google.common`.
    #[test]
    fn resolves_the_curated_residue() {
        assert_eq!(
            resolve("com.google.common.collect.ImmutableList", &[], &[]),
            ImportTarget::External("com.google.guava:guava".to_owned())
        );
    }

    #[test]
    fn resolves_an_internal_package() {
        assert_eq!(
            resolve(
                "com.example.shop.orders.Order",
                &[],
                &["com.example.shop.orders"]
            ),
            ImportTarget::Internal("com.example.shop.orders".to_owned())
        );
    }

    /// Maven's compile classpath is transitive, so an unmatched package is more
    /// likely a transitive reach than a missing declaration. Guessing a coordinate
    /// without an artifactId would be a confident wrong answer.
    #[test]
    fn an_unmatched_package_stays_unresolved() {
        assert!(matches!(
            resolve("net.unknown.thing.Widget", &[], &[]),
            ImportTarget::Unresolved { .. }
        ));
    }

    #[test]
    fn a_prefix_only_matches_on_a_segment_boundary() {
        assert!(matches!(
            resolve("com.exampleX.Thing", &[], &["com.example"]),
            ImportTarget::Unresolved { .. }
        ));
    }
}
