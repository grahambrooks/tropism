# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

gdep is a Rust tool that analyzes a codebase for module and dependency problems. It ships as two
surfaces over the same analysis:

- a **CLI**, for humans and scripts
- an **MCP server**, so coding agents can query the analysis directly

## What it detects

**Source module graph** — relationships between modules within the codebase:

- Circular dependencies

**Manifest hygiene** — what the manifest declares vs. what the source actually imports:

- Unused dependencies (declared but never imported)
- Missing dependencies (imported but never declared)

**External dependency tree** — the resolved graph of third-party packages:

- Diamond dependencies
- Version conflicts
- Dependency bloat

## Core constraint: never invoke native package managers

gdep must **not** shell out to `cargo`, `go`, `pip`, `npm`/`yarn`/`pnpm`, `mvn`/`gradle`, `dotnet`,
`conan`, `swift`, or `bundle`. All dependency data is obtained by reading files in the repository —
using a Rust crate for the format where one fits, or a parser written here where none does.

This is not negotiable, and it drives the design:

- **No network, no toolchain, no build.** gdep works on a checkout with nothing installed, cannot be
  slowed by a resolve step, and cannot execute code from the analyzed repo.
- **Lockfiles are the source of truth for the external tree.** Diamond dependencies, version
  conflicts, and bloat all need a *resolved* graph. Without running a resolver, that graph can only
  come from a lockfile. When a lockfile is absent, gdep reports those checks as unavailable rather
  than guessing — declared ranges in a manifest are not a resolved tree.
- **Manifests give declared intent, lockfiles give resolved reality.** Manifest hygiene checks
  (unused/missing) compare declared deps against imports found in source; they do not need a lockfile.

## Language scope

Ten target languages. Each needs a manifest parser (declared deps), a lockfile parser (resolved
tree), and import extraction from source.

| Language   | Package manager | Manifest                              | Lockfile                                           |
| ---------- | --------------- | ------------------------------------- | -------------------------------------------------- |
| Rust       | Cargo           | `Cargo.toml`                          | `Cargo.lock`                                       |
| Go         | Go modules      | `go.mod`                              | `go.sum`                                           |
| Python     | pip/Poetry/uv   | `requirements.txt`, `pyproject.toml`  | `poetry.lock`, `uv.lock`                           |
| JavaScript | npm/yarn/pnpm   | `package.json`                        | `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml` |
| TypeScript | npm/yarn/pnpm   | `package.json`                        | same as JavaScript                                 |
| Java       | Maven/Gradle    | `pom.xml`, `build.gradle[.kts]`       | `gradle.lockfile` (opt-in; Maven has none)         |
| C#         | NuGet           | `.csproj`, `Directory.Packages.props` | `packages.lock.json` (opt-in)                      |
| C++        | Conan/vcpkg     | `conanfile.txt/.py`, `vcpkg.json`     | `conan.lock`, `vcpkg-configuration.json`           |
| Swift      | SwiftPM         | `Package.swift`                       | `Package.resolved`                                 |
| Ruby       | Bundler         | `Gemfile`                             | `Gemfile.lock`                                     |

### Manifests that are code, not data

`build.gradle[.kts]` (Groovy/Kotlin), `Package.swift` (Swift), and `conanfile.py` (Python) are
programs, not declarative documents. They cannot be fully resolved without executing them, which the
constraint above forbids. Handle these by parsing the common declarative subset and reporting
low confidence when a file uses dynamic constructs — never by executing it.

Note also that Maven has no lockfile, and Gradle/NuGet lockfiles are opt-in and often absent. For
these, resolved-tree checks will frequently be unavailable; the manifest-hygiene checks still work.

## Parsing approach

Prefer a well-maintained crate over a hand-rolled parser. Candidate starting points — verify current
maintenance status on crates.io before committing to any of them: `toml` / `toml_edit` (Cargo.toml,
pyproject.toml), `serde_json` (package.json, package-lock.json, Package.resolved,
packages.lock.json), `quick-xml` (pom.xml, .csproj), a maintained YAML crate for `pnpm-lock.yaml`
(note that `serde_yaml` is archived), `cargo-lock` (Cargo.lock), and `tree-sitter` with per-language
grammars for extracting imports from source.

The remaining formats — `go.mod`/`go.sum`, `yarn.lock`, `Gemfile.lock`, `requirements.txt` — are
bespoke line-oriented formats. Evaluate whether a maintained crate exists before writing a parser;
all are simple enough to parse directly if not.

## Dependency rules

gdep accepts a **ruleset** — `gdep.toml` at the scan root — constraining what may depend on what,
in the manner of NDepend, JDepend, or ArchUnit. **Implemented and enforced on this repository**: see
[gdep.toml](gdep.toml), which `crates/gdep-lang/tests/demos.rs` asserts gdep satisfies.

- **Module rules** — the intended architecture. "The CLI and MCP server must not depend on each
  other, but both may depend on the shared core."
- **Package rules** — approved and discouraged dependencies, optionally scoped to named modules.

Specified in [design/11-dependency-rules.md](design/11-dependency-rules.md). Implemented:
`deny`, `independent`, `allow_only`, package denylists, `allowed_in` scoping, closed-world approved
lists, and stale-rule detection. **Not** implemented — and rejected at parse time with a clear
error rather than silently ignored, so a ruleset never appears to enforce more than it does:
`layers`, `require`, `transitive`, and version constraints.

Rule findings are the only ones besides `cycle` that earn `High` confidence, and they default to
`error` severity because the team asserted them.

This is the strongest part of the product and the recommended next build. A rule violation is the
*presence* of a forbidden import or declaration, which is a fact about a line of source — unlike the
unused-dependency check, which must prove a negative and measured 63% false positives on real
JavaScript. It is also the one thing no native tool can do: `go mod tidy` cannot know an
architecture it was never told about.

## Design specification

`design/` holds the spec — architecture, data model, the language-provider trait, per-check
algorithms, CLI/MCP surfaces, testing strategy, verified crate choices, and open questions. Start at
[design/README.md](design/README.md), and read the relevant document before implementing in that
area: most of it is not re-derivable from the code, because most of it is not built yet.

The build order is at the end of [design/07-open-questions.md](design/07-open-questions.md).

## Layout

```
demo/               deliberately-broken sample projects, one per language; excluded from
                    the cargo workspace, asserted by crates/gdep-lang/tests/demos.rs
crates/gdep-core/   model, discovery, LanguageProvider trait, analyzers, report contract
crates/gdep-lang/   provider implementations, one feature-gated module per language
crates/gdep-cli/    binary `gdep`      — clap front-end, text and JSON renderers
crates/gdep-mcp/    binary `gdep-mcp`  — placeholder until build-order step 6
```

`gdep-core` depends on nothing above it. Analysis logic lives there and nowhere else; both binaries
are adapters.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo test -p gdep-core discovery::          # one module
cargo clippy --workspace --all-targets       # must stay clean
cargo fmt --all
cargo insta accept                           # after intentional renderer changes
cargo build -p gdep-cli --no-default-features # must still build without ratatui

./scripts/demo.sh                            # guided tour across all three languages
./scripts/demo.sh rust                       # one language: go | javascript | rust
./scripts/demo.sh self                       # gdep analyzing gdep
./scripts/demo.sh --tui                      # ...ending in the interactive browser
./target/debug/gdep analyze .                # dogfood directly
```

Output formats: `--format auto` (default; diagnostics on a tty, JSON when piped), `text`, `json`,
`tui`. `auto` never selects `tui` — an alternate screen cannot be piped or read by CI.

Snapshot tests use `insta`. A failing snapshot writes a `.snap.new` beside the original; review the
diff before accepting. The TUI is snapshot-tested through `ratatui`'s `TestBackend`, so it needs no
terminal; both renderers share one fixture report (`render/testdata.rs`) so they cannot drift.

## Current state

The Go vertical slice is complete: discovery, `go.mod` parsing, tree-sitter import extraction,
import resolution, the module graph, and the cycle / unused-dep / missing-dep analyzers. Verified
against Cobra, Zerolog, Prometheus, color, and httprouter with zero false positives.

`version-conflict` and `diamond-dep` report `Unavailable` for Go and always will: `go.sum` records
hashes for the whole module graph rather than the versions MVS selected, and carries no edges, so
there is no resolved tree to analyze without running the Go resolver. `dependency-bloat` is
deferred by design.

A C#/.NET slice is complete: `.csproj` and `packages.lock.json` parsing, tree-sitter-c-sharp
extraction, and namespace-based module identity. Two trait changes were forced by it —
`manifest_extensions`, because `.csproj` is named after the project rather than by convention, and
`ProjectContext::known_modules`, because a `using` names a namespace and the only reliable way to
recognise the solution's own code is to know which namespaces it declares.

A Rust slice is complete and gdep is run against itself — `crates/gdep-lang/tests/demos.rs`
asserts that gdep reports nothing in its own source beyond genuine `Cargo.lock` duplicates.

A JavaScript/TypeScript slice is also complete: `package.json`, `package-lock.json` (a genuinely
resolved graph, unlike `go.sum`), tree-sitter extraction for JS/TS/TSX, and all six checks running.
`version-conflict` and `diamond-dep` execute for the first time here.

Not built: the other six languages, the unimplemented rule kinds above, and the MCP server.

**Before extending the checks, read [design/10-js-evaluation.md](design/10-js-evaluation.md).**
Manifest hygiene (`unused-dep` / `missing-dep`) measured a **63% false-positive rate** on real
JavaScript repositories after three rounds of mitigation, because packages are legitimately used via
HTML `<script src>`, config files, framework strings, and CLI arguments that gdep cannot see without
an installed `node_modules` — which the hermetic constraint forbids. Cycle detection, by contrast,
was sound on every repository. Do not turn hygiene on by default or let it gate CI.

### C# semantics worth knowing

- **A module is the declared namespace, not the path.** Four languages have now needed four
  different file→module strategies (Go: directory; JS: file path; Rust: path→module path; C#:
  the `namespace` declaration read from the file), which is the clearest evidence the mapping
  belongs to the provider rather than the pipeline.
- **`PrivateAssets="all"` is `DepKind::Tooling`.** A Roslyn analyzer participates in the build and
  is never referenced from code — the same shape as an npm package invoked from `scripts`.
- **`System.*` is treated as framework.** In older non-SDK projects some of it shipped as packages,
  so this can hide a genuinely missing reference. The alternative reports a missing dependency on
  `System.Linq`, which is worse.
- **A cross-project cycle is invisible to `cycle`** but caught by a rule, because cycle detection
  runs per project and rules evaluate repo-wide. See `demo/dotnet/README.md`.

### Rust semantics that cost real debugging

Four false-positive classes that only dogfooding surfaced. Each has a regression test in
`crates/gdep-lang/src/rust.rs` and a trap in `demo/rust`:

- **`use` statements are not sufficient to determine crate usage.** Idiomatic Rust writes
  `anyhow::Result<T>` with no import anywhere. Extracting only `use` reported most of this
  workspace's own dependencies as unused. Path references are extracted as
  `ImportForm::PathReference`.
- **A path reference proves usage but never absence.** `Palette::plain()` is a local type, so an
  unrecognised path root resolves to `Unresolved`, never `External` — otherwise every file invents
  missing dependencies.
- **Macro arguments and attribute bodies are flat token trees.** `#[derive(thiserror::Error)]` and
  `eprintln!("{}", gdep_core::report::S)` are real uses that the structured walk cannot see; the
  extractor scans token trees for `identifier ::` pairs.
- **Containment is not dependency.** Rust 2018 uniform paths let `pub use model::X` name a local
  module, and a submodule reaching back with `use super::*` is part of its parent. Modelling either
  as a dependency reported a cycle in `gdep-core` — and would in essentially every Rust crate.

### Go semantics that cost real debugging

Three test-file rules, each forced by a false positive on a real repository. Do not simplify them
without re-running `scripts/demo.sh` and the real-repo check:

- A package's `_test.go` files are **not** compiled in when that package is built as a dependency
  of another package's tests. Attributing their imports to the package invented a 15-module cycle
  in Prometheus.
- `foo/x_test.go` declaring `package foo` (internal test) *can* create a cycle — Go rejects it as
  "import cycle not allowed in test" — so it is checked by reachability from `foo [test]` back to
  `foo`, which is not an SCC of the graph.
- `foo/x_test.go` declaring `package foo_test` (external test) is a separate package that exists
  precisely so it can import `foo`. Never cycle-checked. Cobra, Zerolog, and Prometheus all rely
  on this.

Go's compiler already rejects ordinary import cycles, so `cycle` can only ever fire on Go code that
does not build. See `design/09-product-review.md`.
