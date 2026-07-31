# 03 — Language providers

A provider is everything gdep knows about one language and its package managers. Adding the eleventh
language should mean writing one provider and registering it, touching no analyzer.

## The trait

Defined in `gdep-core` (`provider.rs`); implemented in `gdep-lang`, one feature-gated module per
language, collected by `gdep_lang::registry()`. See [01-architecture.md](01-architecture.md) for why
the trait sits in core rather than beside the implementations.

```rust
trait LanguageProvider {
    fn language(&self) -> Language;

    /// Files whose presence marks a directory as a project root: ["Cargo.toml"].
    fn manifest_names(&self) -> &[&str];

    /// Extensions this provider extracts imports from: ["rs"].
    fn source_extensions(&self) -> &[&str];

    fn parse_manifest(&self, path: &Path, text: &str) -> Result<Vec<DeclaredDep>>;

    /// None when the ecosystem has no lockfile, or none is present.
    fn parse_lockfile(&self, path: &Path, text: &str) -> Result<Vec<ResolvedDep>>;

    /// Every import/require/use in one source file. Pure: same text in, same imports out.
    fn extract_imports(&self, path: &Path, text: &str) -> Result<Vec<Import>>;

    /// The hard one. See below.
    fn resolve_import(&self, import: &Import, ctx: &ProjectContext) -> ImportTarget;

    /// Modules that need no declaration: "std", "os", "fmt", "java.util".
    fn is_stdlib(&self, module: &str) -> bool;

    /// Ecosystem-correct version ordering and requirement matching.
    fn version_ops(&self) -> &dyn VersionOps;
}

enum ImportTarget {
    Internal(ModuleId),           // a module in this project -> module graph edge
    External(PackageName),        // a third-party package   -> checked against declared deps
    Stdlib,                       // ignored by every check
    Unresolved { reason: String },// counted and reported, never silently dropped
}
```

`Unresolved` exists so that resolution failure is measurable. If a provider cannot resolve 40% of a
project's imports, every manifest-hygiene finding for that project is suspect, and the report should
say so rather than emit confident nonsense. **Track the resolution rate per project and downgrade
confidence when it is low.**

## The import→package mapping problem

This is the central difficulty of the whole tool, and it is the thing most likely to make gdep wrong
in practice. The name you import is frequently not the name you declare.

| Language   | Import statement                    | Declared package       |
| ---------- | ----------------------------------- | ---------------------- |
| Python     | `import yaml`                       | `PyYAML`               |
| Python     | `import cv2`                        | `opencv-python`        |
| Python     | `from sklearn import svm`           | `scikit-learn`         |
| JavaScript | `import fp from "lodash/fp"`        | `lodash`               |
| JavaScript | `import x from "@scope/pkg/sub"`    | `@scope/pkg`           |
| Rust       | `use serde_json::Value;`            | `serde-json`           |
| Rust       | `use my_alias::x;`                  | via `package =` rename |
| Java       | `import org.apache.commons.lang3.*` | `commons-lang3`        |
| Ruby       | `require "active_support"`          | `activesupport`        |
| Go         | `import "github.com/x/y/v2/sub"`    | `github.com/x/y/v2`    |

Four distinct mechanisms are needed, in this order of preference:

**1. Structural rules.** Some mappings are deterministic and require no data. Rust normalizes `-` and
`_`. Go's module path is the longest declared prefix of the import path. Scoped npm packages are the
first two path segments; unscoped, the first. Implement these first — they cover most of the volume.

**2. Manifest-declared renames.** Cargo's `package = "..."`, npm aliases (`"a": "npm:b@1"`). Read
from the manifest the provider already parses. Authoritative when present.

**3. Installed-metadata lookup, when present and readable without running anything.** Python's
`.dist-info/RECORD` and `top_level.txt` map import names to distributions authoritatively. If a
virtualenv happens to be in the tree, read it. Never require it, and never create one.

**4. A curated exception table.** For the residue — `yaml`→`PyYAML` and its several hundred friends.
Ship it as data, not code, so it can be updated without a release. Scope it per-ecosystem. Accept
that it will never be complete; that is what `Unresolved` is for.

Where all four fail, emit `Unresolved` with the reason. Do not guess by string similarity — a wrong
mapping produces a confident false "missing dependency", which is worse than an admitted gap.

**Python is the worst case** and should be the proving ground: the import name and distribution name
are genuinely unrelated, one distribution can provide many import names, and imports can be dynamic.
If the design survives Python, the rest follow. **Go is the easiest** and is a good first
implementation to shake out the trait shape.

## Import extraction

Use tree-sitter grammars rather than regex. Imports are syntax, and regex over source gets defeated
by strings, comments, and multi-line forms in every language. The cost is one grammar dependency per
language; the benefit is extraction that is correct by construction and gives real line numbers for
`Evidence`.

Per-language wrinkles that must be handled, not ignored:

- **Conditional compilation** — Rust `#[cfg(...)]`, C++ `#ifdef`. An import behind a disabled cfg is
  still a real dependency. Extract it; do not evaluate the condition.
- **Type-only imports** — TypeScript `import type`. These are erased at runtime but are real
  dev-time dependencies. Record the distinction; let the analyzer decide.
- **Dynamic imports** — `importlib.import_module(name)`, `require(variable)`, reflection. Detect
  the *presence* of dynamic import in a file and use it to cap confidence for that project's
  unused-dependency findings.
- **Re-exports** — a module that exists only to re-export another affects the module graph shape and
  can create cycles that are technically real but useless to report.
- **Relative vs absolute** — always `Internal`, resolved against the project root.

## Per-language starting notes

Beyond the manifest/lockfile table in [CLAUDE.md](../CLAUDE.md):

- **Rust** — workspaces mean multiple manifests share one `Cargo.lock`; `[workspace.dependencies]`
  inheritance must be expanded before hygiene checks run.
- **Go** — `go.sum` contains hashes for modules not in the final build; it is not a resolved tree.
  The real graph must come from `go.mod` `require` blocks including the indirect markers.
- **Python** — multiple manifest formats may coexist and disagree; report which one was used.
- **JS/TS** — the three lockfile formats are unrelated; treat as three parsers. Workspaces/monorepos
  are the norm, not the exception.
- **Java** — Maven has no lockfile and `pom.xml` parent inheritance means a manifest is often
  incomplete on its own without fetching the parent, which gdep will not do. Expect low coverage here
  and say so.
- **C++** — no dominant convention; `CMakeLists.txt` is a program. Lowest expected fidelity of the
  ten.
