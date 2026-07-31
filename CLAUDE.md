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

## Design specification

`design/` holds the spec — architecture, data model, the language-provider trait, per-check
algorithms, CLI/MCP surfaces, testing strategy, verified crate choices, and open questions. Start at
[design/README.md](design/README.md), and read the relevant document before implementing in that
area: most of it is not re-derivable from the code, because most of it is not built yet.

The build order is at the end of [design/07-open-questions.md](design/07-open-questions.md).

## Layout

```
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

./scripts/demo.sh                            # guided tour of the CLI on a fixture repo
./scripts/demo.sh --tui                      # ...ending in the interactive browser
```

Output formats: `--format auto` (default; diagnostics on a tty, JSON when piped), `text`, `json`,
`tui`. `auto` never selects `tui` — an alternate screen cannot be piped or read by CI.

Snapshot tests use `insta`. A failing snapshot writes a `.snap.new` beside the original; review the
diff before accepting. The TUI is snapshot-tested through `ratatui`'s `TestBackend`, so it needs no
terminal; both renderers share one fixture report (`render/testdata.rs`) so they cannot drift.

## Current state

Build-order step 1 is done: workspace, report contract, discovery, and all three renderers, with
tests and a runnable demo script.
No analyzer is implemented, so `gdep analyze` reports every check as `Unavailable` — that path is
wired from the first commit precisely so nobody later mistakes silence for success. The Go provider
declares its manifest and lockfile names (discovery works); its parsing methods return errors.

Next: Go import extraction, then the cycle analyzer.
