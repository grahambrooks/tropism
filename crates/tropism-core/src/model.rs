//! Shared vocabulary: languages, projects, and the two flavours of dependency.
//!
//! See `design/02-data-model.md`.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

/// A language tropism has a provider for.
///
/// **Serialized through [`Language::as_str`], not by a derive.** `rename_all =
/// "kebab-case"` used to produce it, which spelled three of the ten differently in
/// JSON than everywhere else — `java-script`, `type-script`, `c-sharp` — while the
/// text renderer, `tropism workspaces` and `tropism explain` all said `javascript`,
/// `typescript`, `csharp`. Seven languages are single words, so the divergence
/// survived ten language slices and was found by the evaluation harness rather than
/// by anyone reading the output (D40).
///
/// Deriving the wire format from `as_str` is what makes that unrepeatable: there is
/// one spelling because there is one function, and `language_names_agree_across_
/// every_surface` fails if a future variant reintroduces two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    Go,
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    CSharp,
    Cpp,
    Swift,
    Ruby,
}

impl Language {
    /// Every language, so a test can assert a property over all of them.
    pub const ALL: [Language; 10] = [
        Self::Go,
        Self::Rust,
        Self::Python,
        Self::JavaScript,
        Self::TypeScript,
        Self::Java,
        Self::CSharp,
        Self::Cpp,
        Self::Swift,
        Self::Ruby,
    ];

    /// The one spelling. Used by the JSON contract, the text renderer, and
    /// `Display` alike.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Java => "java",
            Self::CSharp => "csharp",
            Self::Cpp => "cpp",
            Self::Swift => "swift",
            Self::Ruby => "ruby",
        }
    }

    /// The inverse of [`Self::as_str`].
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.as_str() == name)
    }
}

impl std::str::FromStr for Language {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::parse(name).ok_or_else(|| format!("unknown language `{name}`"))
    }
}

impl Serialize for Language {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Language {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = <&str>::deserialize(deserializer)?;
        Self::parse(name)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown language `{name}`")))
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A directory containing a recognized manifest: the unit of analysis.
///
/// All paths are relative to the scan root, never absolute — see principle 5 in
/// `design/README.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub root: Utf8PathBuf,
    pub language: Language,
    pub manifests: Vec<Utf8PathBuf>,
    pub lockfile: Option<Utf8PathBuf>,
}

/// How a dependency is used. Analyzers comparing declarations against imports must
/// be `DepKind`-aware, or every dev-dependency looks unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DepKind {
    Runtime,
    Dev,
    Build,
    Optional,
    Peer,
    /// Declared but not directly imported — pulled in by another dependency and
    /// recorded for reproducibility. Go's `// indirect` markers are the clearest
    /// case. Reporting these as unused would be a false-positive storm, so the
    /// unused-dependency analyzer skips them.
    Indirect,
    /// Declared to be *invoked or consumed by a build step*, not imported: a CLI
    /// run from a script (`eslint`, `rollup`), or ambient type definitions the
    /// compiler reads without any import (`@types/*`).
    ///
    /// Measured, not assumed: before this existed, every one of Chalk's six
    /// findings and 25 of Zustand's 27 were packages of exactly this shape.
    Tooling,
}

impl DepKind {
    /// Whether an absent import makes this dependency suspicious. False for the
    /// kinds that are *expected* to have no import.
    pub fn expects_direct_import(self) -> bool {
        !matches!(self, Self::Indirect | Self::Tooling)
    }
}

/// A parsed manifest: its declared dependencies plus the package's own identity.
///
/// The identity matters more than it looks. Go needs the `module` line to tell an
/// internal import from an external one, and there is no way to recover it from the
/// dependency list alone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub deps: Vec<DeclaredDep>,
    pub package_name: Option<String>,
}

/// A dependency as the manifest declares it: a name and a version *requirement*.
///
/// The requirement stays a raw string; interpreting it is `VersionOps`' job, because
/// SemVer, PEP 440, Maven, and RubyGems orderings all differ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredDep {
    pub name: String,
    pub requirement: String,
    pub kind: DepKind,
    pub declared_at: Provenance,
}

/// A dependency as the lockfile resolved it: a name and an exact version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedDep {
    /// Identifies this *copy*, not this package. npm may install the same name at
    /// several versions in different places, and `dependencies` edges point at a
    /// specific copy — so the name alone cannot be the identity.
    pub key: String,
    pub name: String,
    pub version: String,
    /// Keys of the copies this one resolves to.
    pub dependencies: Vec<String>,
}

/// Where a fact came from. Every finding must trace back to one of these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub file: Utf8PathBuf,
    pub line: Option<u32>,
}

impl Provenance {
    pub fn new(file: impl Into<Utf8PathBuf>, line: Option<u32>) -> Self {
        Self {
            file: file.into(),
            line,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D40. The JSON contract and every human-facing surface must spell a language
    /// the same way.
    ///
    /// `05-interfaces.md` promises that any question answerable by the CLI is
    /// answerable over MCP *with the same result*, and both go through this enum.
    /// Before this test, `rename_all = "kebab-case"` made the contract say
    /// `java-script` while `Display` said `javascript` — one tool, two vocabularies,
    /// undetected across ten language slices because seven of the ten are single
    /// words.
    #[test]
    fn language_names_agree_across_every_surface() {
        for language in Language::ALL {
            let json = serde_json::to_string(&language).expect("serializes");
            assert_eq!(
                json,
                format!("\"{}\"", language.as_str()),
                "{language:?}: the JSON contract and as_str disagree"
            );
            assert_eq!(
                json,
                format!("\"{language}\""),
                "{language:?}: Display disagrees"
            );
        }
    }

    /// A name that went out must come back as the same value, or a consumer cannot
    /// round-trip a report through the contract.
    #[test]
    fn language_round_trips_through_json() {
        for language in Language::ALL {
            let json = serde_json::to_string(&language).unwrap();
            let back: Language = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(back, language);
            assert_eq!(Language::parse(language.as_str()), Some(language));
        }
    }

    /// The old spellings are gone rather than accepted alongside the new ones.
    /// Silently accepting both would leave two vocabularies in the contract, which
    /// is the defect rather than the fix.
    #[test]
    fn the_hyphenated_spellings_are_rejected() {
        for stale in ["java-script", "type-script", "c-sharp"] {
            assert!(
                Language::parse(stale).is_none(),
                "`{stale}` must no longer be a language name"
            );
            assert!(serde_json::from_str::<Language>(&format!("\"{stale}\"")).is_err());
        }
    }

    /// `ALL` is what the tests above quantify over, so a language missing from it
    /// would silently exempt itself from every one of them.
    #[test]
    fn every_language_is_in_all() {
        let mut names: Vec<&str> = Language::ALL.iter().map(|l| l.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            Language::ALL.len(),
            "duplicate or missing entry"
        );
        // Mirrors the ten in CLAUDE.md's language table.
        assert_eq!(Language::ALL.len(), 10);
    }
}
