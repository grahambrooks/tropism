# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

tropism is a Rust tool that enforces a team's architecture rules — and analyzes module and dependency
problems — across ten languages, without invoking a package manager or building anything.

> **One ruleset, enforced at commit time and over the whole repository, across ten languages, with
> no build and no install.**

Two scopes over one analysis and one ruleset:

- **`tropism check <files>`** — a change, at commit time, through a pre-commit hook. Rules only,
  scoped to the files given. See [design/14-incremental-checking.md](design/14-incremental-checking.md).
- **`tropism analyze`** — the whole repository, in CI or by hand. Every check.

A third surface, an **MCP server** for coding agents, is specified and deliberately last — see
"Dependency rules" below for why it moved.

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

tropism must **not** shell out to `cargo`, `go`, `pip`, `npm`/`yarn`/`pnpm`, `mvn`/`gradle`, `dotnet`,
`conan`, `swift`, or `bundle`. All dependency data is obtained by reading files in the repository —
using a Rust crate for the format where one fits, or a parser written here where none does.

This is not negotiable, and it drives the design:

- **No network, no toolchain, no build.** tropism works on a checkout with nothing installed, cannot be
  slowed by a resolve step, and cannot execute code from the analyzed repo.
- **Lockfiles are the source of truth for the external tree.** Diamond dependencies, version
  conflicts, and bloat all need a *resolved* graph. Without running a resolver, that graph can only
  come from a lockfile. When a lockfile is absent, tropism reports those checks as unavailable rather
  than guessing — declared ranges in a manifest are not a resolved tree.
- **Manifests give declared intent, lockfiles give resolved reality.** Manifest hygiene checks
  (unused/missing) compare declared deps against imports found in source; they do not need a lockfile.

## Language scope

Ten target languages, **all implemented**. Each needs a manifest parser (declared deps), a lockfile
parser (resolved tree), and import extraction from source.

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

Only Cargo and npm produce a genuinely resolved *tree* — exact versions and edges. `uv.lock`,
`poetry.lock`, and `Gemfile.lock` have both but describe a flat environment, so a diamond cannot
occur in them. `go.sum`, `gradle.lockfile`, `Package.resolved`, and `conan.lock` have versions and
no edges at all, and Maven has no lockfile whatsoever; those four report `Unavailable` with a
reason naming the file.

### Manifests that are code, not data

`Gemfile` (Ruby), `build.gradle[.kts]` (Groovy/Kotlin), `Package.swift` (Swift), and `conanfile.py`
(Python) are programs, not declarative documents. They cannot be fully resolved without executing
them, which the constraint above forbids. All four are implemented by parsing the declarative subset
**with a grammar** — the Ruby, Swift, and Python grammars respectively, and a line parser for Gradle
— never by executing the file.

The rule when a construct is dynamic is to contribute *nothing*, not to guess: `gem "rails-#{v}"`,
`.package(url: "\(base)/y.git")`, `self.requires(f"fmt/{self.version}")`, and
`implementation libs.guava` all yield no dependency, because a package name that does not exist is
worse in a report than one missing from it. Each has a regression test. Gradle is the single
deliberate exception — `implementation "org.x:y:$version"` has a real coordinate wrapped around an
unknowable version, so the coordinate is kept and only the version dropped.

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

tropism accepts a **ruleset** — `tropism.toml` at the scan root — constraining what may depend on what,
in the manner of NDepend, JDepend, or ArchUnit. **Implemented and enforced on this repository**: see
[tropism.toml](tropism.toml), which `crates/tropism-lang/tests/demos.rs` asserts tropism satisfies.

- **Module rules** — the intended architecture. "The CLI and MCP server must not depend on each
  other, but both may depend on the shared core."
- **Package rules** — approved and discouraged dependencies, optionally scoped to named modules.

Specified in [design/11-dependency-rules.md](design/11-dependency-rules.md). Implemented:
`deny`, `independent`, `allow_only`, package denylists, `allowed_in` scoping, closed-world approved
lists, and stale-rule detection. **Not** implemented — and rejected at parse time with a clear
error rather than silently ignored, so a ruleset never appears to enforce more than it does:
`layers`, `require`, `transitive`, and version constraints.

`tropism.toml` also carries `exclude` globs, applied before discovery, which is what lets
`tropism analyze . --fail-on error` pass on this repository despite `demo/` and `tests/fixtures/`
being deliberately broken. Exclusions are disclosed in every report with a match count — an
exclusion is a blind spot, and a silent blind spot is the failure mode `CheckStatus` exists to
prevent. **Never widen an exclusion to make the gate pass.**

It also carries `[[workspaces]]`, which draws workspace boundaries where an ecosystem states none.
See "Workspace boundaries" below.

Rule findings are the only ones besides `cycle` that earn `High` confidence, and they default to
`error` severity because the team asserted them.

**This is the product.** A rule violation is the *presence* of a forbidden import or declaration,
which is a fact about a line of source — unlike the unused-dependency check, which must prove a
negative and measured 63% false positives on real JavaScript. It is also the one thing no native
tool can do: `go mod tidy` cannot know an architecture it was never told about.

The thesis, arrived at by elimination and each elimination measured:

> **One ruleset, enforced at commit time and over the whole repository, across ten languages, with
> no build and no install.**

Two scopes, one ruleset: `tropism check <files>` on a change and `tropism analyze` on everything.
Protect that symmetry — a rule only one scope can evaluate would be worse than no rule, because it
would make the hook a liar. The advisory checks (cycle, hygiene, the resolved tree) belong to the
whole-repository run, where a human reads the output before acting; only what is safe to enforce
automatically runs at commit time.

`tropism check` and the release pipeline are both built, so the hook is real and binaries ship. The
MCP server was once the recommended next build and is now last: its flagship query needs a resolved
tree, and four of the ten ecosystems have no lockfile edges — see the revision at the end of
[design/09-product-review.md](design/09-product-review.md).

**Scoping is the part to understand before changing `check`.** A violation is an edge, an edge has
two ends, and the violation belongs to the file at the *source* end. If `api/user.ts` gains an import
of `data/db`, that is attributable to `api/user.ts`; if `data/db.ts` changed and something in `api`
already imported it, the edge is not new and is not this commit's fault. That asymmetry is the whole
ratchet, and `crates/tropism-lang/tests/check.rs` pins it from both directions.

## Build and release

Releases are cut by **`make release`** and built by **`dist` (cargo-dist)**. Never bump the version
by hand: the Makefile computes CalVer from the tag count, writes it to `Cargo.toml`, refreshes
`Cargo.lock`, commits, tags, and pushes. The **tag** is what starts the release.

```sh
make release-dry     # what it would do
make release         # do it — runs `make check` first and refuses a dirty tree
```

Two things to know before touching any of it:

- **`dist` owns `.github/workflows/release.yml` and regenerates it.** Anything hand-added there is
  lost on the next `dist generate`. Config lives in `dist-workspace.toml`; crates.io publishing
  lives in its own `publish-crate.yml` for exactly this reason, and stays manual because a
  crates.io version can be yanked but never reused.
- **The version is committed, not injected.** That reverses the original design, deliberately —
  [design/15-dist-evaluation.md](design/15-dist-evaluation.md) records the evaluation and why
  adoption overruled it. The short version: judged on maintainer ergonomics the old pipeline won
  because it already existed; judged on *user* ergonomics, which is what decides adoption, a
  one-line installer that needs no admin was never close.

**tropism is not a pure-Rust binary.** Every tree-sitter grammar compiles C, so cross-compilation
needs a cross C toolchain and each target is built on a native runner. dist 0.32 can cross-compile
via zigbuild/xwin; that is untested here and the native-runner matrix is deliberate.

**The name is settled, but re-verify before the first crates.io publish.** `tropism`,
`tropism-core`, `tropism-lang`, and `tropism-mcp` were all free as of 2026-07-31, so the crate, the
binary, and the command are all `tropism`. Nothing reserves a name, and nothing has been published
yet — `publish-crate.yml` has never run.

The highest-value distribution work remaining is **code signing**, not more channels — specified in
[design/16-signing.md](design/16-signing.md). Build provenance attestation already ships and is
*not* the same thing: it proves where an artifact came from, and answers nothing the operating
system asks before running it.

**Dependabot alerts on `demo/` are noise by construction** and cannot be filtered by path — alerts
come from the dependency graph, which finds every manifest by filename, and auto-triage rules match
severity and package but not directory. `make alerts` splits the real count from the fixtures;
`./scripts/dismiss-demo-alerts.sh --apply` dismisses the fixtures as `not_used`, which is accurate:
nothing under `demo/` is ever installed. Never "fix" a demo dependency — the pinned, conflicting,
outdated versions are what the samples demonstrate, and `demos.rs` asserts them.

## Workspace boundaries

A **workspace** is the set of projects that may import each other's published names *without
declaring them*. It decides what `missing-dep` reports, so a wrong boundary is a silent wrong answer.
`crates/tropism-core/src/workspace.rs` owns it; membership is established three ways, most
authoritative first: `configured` (`[[workspaces]]` in `tropism.toml`), `declared` (Cargo `members`,
npm `workspaces`, `pnpm-workspace.yaml`, `go.work`, Maven `<modules>`, Gradle `include`), and
`language` (everything unclaimed, grouped by language — the fallback for Python, Ruby, Swift, C++,
and NuGet, which declare nothing).

Four things not to undo:

- **Language is checked in addition to membership, never instead of it.** No workspace declaration
  makes a Rust crate importable from Node. This is what keeps the scheme a strict narrowing of the
  old repo-wide sibling set, so it cannot introduce a false positive that set did not already have.
- **Ancestor hoisting is bounded by the directory tree, not by the workspace**, because Node's
  resolution walks up parent directories whether or not a workspace is involved.
- **The exemption is disclosed, never silent** — `ProjectReport::sibling_exemptions`, with the
  package, the supplying project, and the import count. Same reasoning as `exclude` match counts.
- **A crossing is a rule, not an inferred check.** `crosses_workspace = true` lets a team assert it
  at High confidence; tropism does not guess a severity for it.

This resolved open question 1 in `design/07-open-questions.md`, and the resolution is worth reading
before touching any of it: the graph half and the hygiene half of that question had shipped
*disagreeing about the same edge*, and the disagreement was only visible by testing them against
each other. `crates/tropism-lang/tests/workspaces.rs` pins both leaks closed.

## Cycle scopes

Cycle findings carry `details.scope`:

- **`module`** — within one project, from that project's module graph.
- **`project`** — between projects, from the repo-wide edges the rule engine collects.

Both are needed. A cycle spanning two packages used to report `ok`, which is the silent-clean
failure the rest of the tool is built to avoid. `demo/dotnet` carries one of each.

## Known limitations

**[design/12-known-limitations.md](design/12-known-limitations.md) is the register.** Before
"fixing" something that looks broken, check whether it is structural — a consequence of never
invoking a package manager, which gets reported rather than resolved — or genuinely deferred. The
document separates the two, because conflating them is how the core constraint gets traded away by
accident.

Add to it rather than rediscovering the same gap.

The most important entry to read before touching the resolved-tree checks is **S8**: a lockfile is
resolved for every feature combination and every target platform and records neither, so
`version-conflict` and `diamond-dep` describe the *lockfile*, not the build. Dogfooding measures the
gap — 17 findings against this repository, all correct about `Cargo.lock`, against three duplicate
sets that `cargo tree --duplicates` says actually compile. The rest come from an optional `ratatui`
backend that is never enabled and from a UEFI-only crate. Nothing is wrong with the lockfile and
`cargo update` changes none of it.

## Design specification

`design/` holds the spec — architecture, data model, the language-provider trait, per-check
algorithms, CLI/MCP surfaces, testing strategy, verified crate choices, and open questions. Start at
[design/README.md](design/README.md), and read the relevant document before implementing in that
area: much of it is not re-derivable from the code, and the parts still unbuilt (the MCP server, the
remaining rule kinds) exist only there.

The build order is at the end of [design/07-open-questions.md](design/07-open-questions.md).

## Layout

```
demo/               deliberately-broken sample projects, one per language (go, javascript, rust,
                    dotnet, python, ruby, java, swift, cpp); excluded from the cargo workspace,
                    asserted by crates/tropism-lang/tests/demos.rs
crates/tropism-core/   model, discovery, LanguageProvider trait, analyzers, report contract
crates/tropism-lang/   provider implementations, one feature-gated module per language — go,
                    javascript, rust, csharp, python, ruby, java, swift, cpp
crates/tropism/    binary `tropism`      — clap front-end, text and JSON renderers
crates/tropism-mcp/    binary `tropism-mcp`  — placeholder until build-order step 6
```

`tropism-core` depends on nothing above it. Analysis logic lives there and nowhere else; both binaries
are adapters.

## Commands

```sh
cargo build --workspace
cargo test --workspace
cargo test -p tropism-core discovery::          # one module
cargo clippy --workspace --all-targets       # must stay clean
cargo fmt --all
cargo insta accept                           # after intentional renderer changes
cargo build -p tropism --no-default-features # must still build without ratatui

./scripts/demo.sh                            # guided tour across every language
./scripts/demo.sh rust                       # one: go | javascript | rust | dotnet
                                             #      python | ruby | java | swift | cpp
./scripts/demo.sh self                       # tropism analyzing tropism
./scripts/demo.sh --tui                      # ...ending in the interactive browser
./target/debug/tropism analyze .                # dogfood directly
./target/debug/tropism check src/a.rs           # rules only, scoped to a change
./target/debug/tropism workspaces .             # boundaries, their provenance, and what crosses them
./target/debug/tropism explain src/a.rs         # why each import in one file classified as it did

prek install                                 # wire the hooks (prek.toml)
prek run tropism -a                          # the commit-time hook, without committing
prek run tropism-all --hook-stage pre-push -a   # the whole-repository one
```

Output formats: `--format auto` (default; diagnostics on a tty, JSON when piped), `text`, `json`,
`tui`. `auto` never selects `tui` — an alternate screen cannot be piped or read by CI.

Snapshot tests use `insta`. A failing snapshot writes a `.snap.new` beside the original; review the
diff before accepting. The TUI is snapshot-tested through `ratatui`'s `TestBackend`, so it needs no
terminal; both renderers share one fixture report (`render/testdata.rs`) so they cannot drift.
**A snapshot must never contain an absolute path** — the scan root is substituted with
`STABLE_SCAN_ROOT` for exactly that reason, after a committed snapshot holding one developer's home
directory failed CI on all three operating systems.

**The pre-commit hook runs the same gate as CI.** [prek.toml](prek.toml) carries a `repo: local`
hook running `tropism analyze . --format text --fail-on error`. Two things not to change without
reading [design/14-incremental-checking.md](design/14-incremental-checking.md): it must stay
`--format text`, because prek captures stdout and `auto` would emit JSON at the moment a developer
needs the rule's `reason`; and it must never gate on `unused-dep`. It runs the whole repository
because `tropism check <files>` is not built (D24) — that, not a faster whole-repo run, is the fix
when this becomes slow.

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

A Rust slice is complete and tropism is run against itself — `crates/tropism-lang/tests/demos.rs`
asserts that tropism reports nothing in its own source beyond genuine `Cargo.lock` duplicates.

A JavaScript/TypeScript slice is also complete: `package.json`, `package-lock.json` (a genuinely
resolved graph, unlike `go.sum`), tree-sitter extraction for JS/TS/TSX, and all six checks running.
`version-conflict` and `diamond-dep` execute for the first time here.

All ten target languages are now built. The five added last — Python, Ruby, Java, Swift, and C++ —
each have a provider, a demo under `demo/`, and assertions in `crates/tropism-lang/tests/demos.rs`.
No trait change was needed for any of them, which is the first real evidence that
`LanguageProvider` is the right shape.

`tropism check [FILES...]` is built, with `--staged` and `--since <ref>`, and `.pre-commit-hooks.yaml`
ships so other repositories can consume the hook. The release pipeline cuts CalVer binaries for six
targets on every green push to main, with checksums and provenance; crates.io publishing is wired but
deliberately manual.

Workspace boundaries are first-class: inferred from each ecosystem's own declaration, overridable
with `[[workspaces]]`, inspectable with `tropism workspaces`, and enforceable with a
`crosses_workspace` rule. This closed open question 1 and D16, and added `LanguageProvider::
workspace_files` / `workspace_members` — the first trait change since it converged across the last
five languages, and a deliberate one.

`check` is parse-incremental (D36): only the changed files are extracted, and 3,000 files check in
0.05 s. Extraction runs on `rayon` (D4), order-preserving so output stays byte-identical.
`tsconfig.json` `paths` aliases resolve (D15). A workspace member now names the lockfile that covers
it instead of saying "no lockfile found" (D2).

**Performance has one hard-won rule: measure, do not reason.** D4 was implemented and moved a
3,000-file run from 19.5 s to 19.5 s, which is what revealed D39 — Rust import resolution rebuilt the
project's module set on *every* import, quadratically. Fixing that took the same run to 0.11 s, and
only then was `rayon` worth anything (0.30 s serial → 0.08 s). A comment in `ProjectContext` had
claimed the memoization existed for years; it did not.

The audit that followed found the same shape — O(project) work per import — in **JavaScript**
(scanning `source_files` per relative import), **Python/Java/C#** (`longest_prefix` walking every
module), and **C++** (a suffix `find` over every module). All fixed; see D39 for the table. Go, Ruby
and Swift were already `contains` on a set. **Nothing guards against reintroducing this**, so when
touching `resolve_import`, check that per-import work is bounded by the *import*, not by the project.

Two rules for benchmarking it. Verify output is byte-identical before claiming a speedup — a speed
fix must not change an answer. And benchmark on an *acyclic* fixture: a synthetic repo whose imports
wrap modulo n is one giant SCC, and the cycle analyzer's cost over it will masquerade as a
resolution problem.

Not built, in priority order: a baseline for whole-repository runs (D8, designed in
design/17-baselines.md), the unimplemented rule kinds above, an `undeclared-sibling` check (D38 —
measure against the ten-repo corpus before building), and the MCP server.

**Before extending the checks, read [design/10-js-evaluation.md](design/10-js-evaluation.md).**
Manifest hygiene (`unused-dep` / `missing-dep`) measured a **63% false-positive rate** on real
JavaScript repositories after three rounds of mitigation, because packages are legitimately used via
HTML `<script src>`, config files, framework strings, and CLI arguments that tropism cannot see without
an installed `node_modules` — which the hermetic constraint forbids. Cycle detection, by contrast,
was sound on every repository. Do not turn hygiene on by default or let it gate CI.

### What the last five languages taught

- **Nine file→module strategies for ten languages.** Go: directory. JS and Ruby: file path. Rust:
  path→module path. C# and Java: the declaration in the source. Python: dotted path with `src/`
  stripped and `__init__` collapsed. Swift: the *target*, which is a name in the manifest. C++: the
  component — path with the include-path root stripped and the extension dropped, so a header and
  its translation unit are one module. The mapping belongs to the provider; the pipeline should
  never assume it.
- **`import` order matters when translating names.** The Python provider translates an import name
  to a distribution *before* comparing against the manifest. Doing it the other way round makes
  `import yaml` miss a declared `PyYAML`, and the project then collects a false missing-dep on
  `pyyaml` and a false unused-dep on `PyYAML` — two findings, both wrong, from one correct line.
  There is a regression test named after this.
- **Swift is the only ecosystem that solves import→package itself.** The manifest states the
  mapping, in the target that uses it: `.product(name: "Logging", package: "swift-log")`. So the
  Swift provider carries no curated table, and records a dependency under its *product* name — with
  one exception, a package no target uses, which keeps its own identity so `unused-dep` can report
  it.
- **A flat environment cannot have a diamond.** Python, Ruby, and Swift install one version of each
  package, so `diamond-dep` runs and correctly finds nothing. Do not "fix" this. What a fork in a
  `uv.lock` means is a platform-conditional resolution, which is a version conflict and never a
  diamond — and an edge naming a forked package is ambiguous, so the Python provider drops it rather
  than attaching it to an arbitrary copy.
- **Only Cargo and npm produce a real resolved tree.** Maven has no lockfile at all;
  `gradle.lockfile`, `Package.resolved`, and `conan.lock` are flat lists with no edges. Each returns
  `Ok(None)` from `parse_lockfile` with a `resolved_tree_note`, exactly as Go does for `go.sum`.
  Returning the flat list would let `diamond-dep` report a confident `0 findings` about a graph it
  never had.
- **Two grammars do double duty.** `conanfile.py` is parsed with the Python grammar and the
  `Gemfile` with the Ruby one. That is what makes "manifests that are code" tractable without a
  bespoke parser per format, and neither file is ever executed.

### C# semantics worth knowing

- **A module is the declared namespace, not the path.** The first four languages needed four
  different file→module strategies, and the last five added four more — see above.
- **`PrivateAssets="all"` is `DepKind::Tooling`.** A Roslyn analyzer participates in the build and
  is never referenced from code — the same shape as an npm package invoked from `scripts`.
- **`System.*` is treated as framework.** In older non-SDK projects some of it shipped as packages,
  so this can hide a genuinely missing reference. The alternative reports a missing dependency on
  `System.Linq`, which is worse.
- **A cross-project cycle is invisible to `cycle`** but caught by a rule, because cycle detection
  runs per project and rules evaluate repo-wide. See `demo/dotnet/README.md`.

### Rust semantics that cost real debugging

Four false-positive classes that only dogfooding surfaced. Each has a regression test in
`crates/tropism-lang/src/rust.rs` and a trap in `demo/rust`:

- **`use` statements are not sufficient to determine crate usage.** Idiomatic Rust writes
  `anyhow::Result<T>` with no import anywhere. Extracting only `use` reported most of this
  workspace's own dependencies as unused. Path references are extracted as
  `ImportForm::PathReference`.
- **A path reference proves usage but never absence.** `Palette::plain()` is a local type, so an
  unrecognised path root resolves to `Unresolved`, never `External` — otherwise every file invents
  missing dependencies.
- **Macro arguments and attribute bodies are flat token trees.** `#[derive(thiserror::Error)]` and
  `eprintln!("{}", tropism_core::report::S)` are real uses that the structured walk cannot see; the
  extractor scans token trees for `identifier ::` pairs.
- **Containment is not dependency.** Rust 2018 uniform paths let `pub use model::X` name a local
  module, and a submodule reaching back with `use super::*` is part of its parent. Modelling either
  as a dependency reported a cycle in `tropism-core` — and would in essentially every Rust crate.

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
