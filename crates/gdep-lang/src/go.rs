//! Go provider.
//!
//! First implementation by design: Go's import→package mapping is the simplest of
//! the ten (longest declared module-path prefix wins), so it shakes out the trait
//! shape without the resolution problem dominating. See
//! `design/07-open-questions.md`, build order.
//!
//! Discovery is wired up; parsing is not yet implemented.

use camino::Utf8Path;
use gdep_core::model::{DeclaredDep, Language, ResolvedDep};
use gdep_core::provider::{Import, LanguageProvider, VersionOps};

pub struct GoProvider;

/// Go's standard library top-level packages. Imports without a dot in their first
/// path segment are stdlib by construction, which covers this list and more; the
/// explicit set is kept for clarity and for the ambiguous cases.
const STDLIB_PREFIXES: &[&str] = &[
    "archive",
    "bufio",
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

/// Go module versions are SemVer with a leading `v`, but comparison is not needed
/// until the version-conflict analyzer lands.
struct GoVersionOps;

impl VersionOps for GoVersionOps {
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

    /// `go.sum` holds hashes for modules that may not be in the final build, so it is
    /// not a resolved tree on its own — see `design/03-language-providers.md`. It is
    /// still the marker that resolution has happened.
    fn lockfile_names(&self) -> &'static [&'static str] {
        &["go.sum"]
    }

    fn source_extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn parse_manifest(&self, _path: &Utf8Path, _text: &str) -> anyhow::Result<Vec<DeclaredDep>> {
        anyhow::bail!("go.mod parsing is not implemented yet")
    }

    fn parse_lockfile(
        &self,
        _path: &Utf8Path,
        _text: &str,
    ) -> anyhow::Result<Option<Vec<ResolvedDep>>> {
        anyhow::bail!("go.sum parsing is not implemented yet")
    }

    fn extract_imports(&self, _path: &Utf8Path, _text: &str) -> anyhow::Result<Vec<Import>> {
        anyhow::bail!("Go import extraction is not implemented yet")
    }

    fn is_stdlib(&self, module: &str) -> bool {
        let root = module.split('/').next().unwrap_or(module);
        // A first segment containing a dot is a domain, so a third-party module.
        !root.contains('.') && STDLIB_PREFIXES.contains(&root)
    }

    fn version_ops(&self) -> &dyn VersionOps {
        &GoVersionOps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_stdlib_imports() {
        let go = GoProvider;
        assert!(go.is_stdlib("fmt"));
        assert!(go.is_stdlib("net/http"));
        assert!(go.is_stdlib("encoding/json"));
    }

    #[test]
    fn rejects_third_party_imports() {
        let go = GoProvider;
        assert!(!go.is_stdlib("github.com/spf13/cobra"));
        assert!(!go.is_stdlib("golang.org/x/sync/errgroup"));
        assert!(!go.is_stdlib("example.com/internal/db"));
    }
}
