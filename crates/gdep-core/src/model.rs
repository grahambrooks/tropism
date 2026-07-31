//! Shared vocabulary: languages, projects, and the two flavours of dependency.
//!
//! See `design/02-data-model.md`.

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
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
}

impl DepKind {
    /// Whether an absent import makes this dependency suspicious. False for
    /// `Indirect`, which is *expected* to have no import.
    pub fn expects_direct_import(self) -> bool {
        !matches!(self, Self::Indirect)
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
    pub name: String,
    pub version: String,
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
