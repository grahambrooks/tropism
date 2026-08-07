# 12 — Known limitations

Every limitation found while building the ten language providers and the rule engine, in one place.
Each entry says what it costs and what it would take to fix.

The register splits into two halves, and the split is the important part:

- **Structural** — consequences of the core constraint that tropism never invokes a package manager and
  never executes the repository. These do not get "fixed"; they get *reported honestly*. Anything
  that would resolve them trades away the property that makes tropism worth having.
- **Deferred** — things that are simply not built yet, with no obstacle beyond effort.

Confusing the two is the main risk this document exists to prevent.

---

## Structural

### S1. Proving a dependency is *unused* is not possible offline

`unused-dep` measured a **63% false-positive rate** on ten real JavaScript repositories after three
rounds of mitigation ([10-js-evaluation.md](10-js-evaluation.md)). Packages are genuinely used
through channels tropism cannot see: HTML `<script src>` tags, `vitest.config.mts`, `tsconfig`
`extends`, GitHub Actions workflows, framework strings (`app.set('view engine', 'hbs')`), and spawn
arguments (`--import=tsx/esm`).

The deepest instance: `gzip-size-cli` provides the `gzip-size` binary and `npm-run-all2` provides
`run-p`. Mapping a script command back to its package needs `bin` fields from an installed
`node_modules`.

**Cost:** the check cannot be trusted, and must never gate CI.
**Why it stays:** every fix requires either an installed dependency tree or executing config files.
**Mitigation:** cap confidence, default it off, and say plainly in the message what is invisible.

### S2. A path reference proves usage but never absence

Rust writes `anyhow::Result<T>` with no `use` statement, so path references must count as usage
(S1's inverse). But `Palette::plain()` is a local type, so an unrecognised path root can never be
reported as a *missing* dependency — `ImportForm::PathReference` exists to enforce that asymmetry.

**Cost:** a crate used only through fully-qualified paths and never declared goes unreported.
`demo/rust/README.md` documents this deliberately.
**Why it stays:** the alternative invents a missing dependency on every local type.

### S3. Resolved-tree checks are unavailable for most of the ten target languages

`version-conflict` and `diamond-dep` need a resolved graph — exact versions *and* edges. Most
ecosystems ship a lockfile with only one of the two.

| Ecosystem | Lockfile                          | Resolved graph?                                                             |
| --------- | --------------------------------- | --------------------------------------------------------------------------- |
| npm       | `package-lock.json`               | yes                                                                         |
| Cargo     | `Cargo.lock`                      | yes                                                                         |
| Python    | `uv.lock`, `poetry.lock`          | yes — but the environment is flat, see S7                                   |
| Ruby      | `Gemfile.lock`                    | yes — but Bundler resolves flat, see S7                                     |
| .NET      | `packages.lock.json`              | yes, but opt-in and usually absent                                          |
| Go        | `go.sum`                          | **no** — hashes for the whole module graph, no edges, not the MVS selection |
| Maven     | none                              | **no** — there is no lockfile at all                                        |
| Gradle    | `gradle.lockfile`                 | **no** — versions per configuration, no edges; also opt-in                  |
| Swift     | `Package.resolved`                | **no** — a flat list of pinned packages, no edges                           |
| C++       | `conan.lock`, vcpkg baseline      | **no** — flat pinned references; vcpkg pins a registry commit, not a graph  |

**Cost:** on five of the ten ecosystems, two of the advertised checks never run.
**Why it stays:** obtaining the tree means running the resolver.
**Mitigation:** `CheckStatus::Unavailable` with the ecosystem-specific reason, so silence is never
mistaken for a clean result.

### S4. Manifests that are programs are read incompletely

`Gemfile`, `build.gradle[.kts]`, `Package.swift`, `conanfile.py`, and `vitest.config.mts` are code.
tropism parses the declarative subset and cannot see anything dynamic.

Every implemented case skips rather than guesses, and each has a regression test:
`gem "rails-#{variant}"`, `.package(url: "\(base)/y.git")`, `self.requires(f"fmt/{self.version}")`,
and `implementation libs.guava` all contribute *nothing*, because a package name that does not exist
is worse in a report than a package that is missing from it. Gradle is the one exception, and it is
deliberate: `implementation "org.x:y:$version"` has a real coordinate around an unknowable version,
so the coordinate is kept and only the version is dropped.

**Why it stays:** principle 1 — tropism never executes the repository it analyzes. This is a security
property for the agent use case, not a convenience.

### S5. Import name ≠ package name, and the gap has no complete answer

`import yaml` means `PyYAML`; `using Xunit` means `xunit`; `com.google.common` means
`com.google.guava:guava`. Structural rules and a curated exception table cover most of it; the
residue stays `Unresolved` rather than guessing.

Now measured across all ten. The gap is widest in Java, where a coordinate
(`groupId:artifactId`) and an import (a package) are different namespaces with only a convention
between them — and it closes entirely in exactly one ecosystem: **Swift states the mapping in the
manifest**, in the target that uses the product (`.product(name: "Logging", package: "swift-log")`),
so that provider needs no table at all.

Java is also the one language where an unmatched import stays `Unresolved` rather than becoming a
missing-dep finding. Maven puts a dependency's own dependencies on the compile classpath, so code can
import a transitive artifact and compile cleanly; a coordinate guessed from a package prefix would
have no artifactId in it.

**Cost:** unresolved imports cap hygiene confidence for the whole project, and Java reports fewer
missing dependencies than exist.
**Why it stays:** the authoritative mapping lives in installed package metadata.

### S6. License policy is out of scope

`cargo-deny`-style licence checks need each dependency's licence metadata, which lives in the
registry or an installed tree. Stated in [11-dependency-rules.md](11-dependency-rules.md) rather
than half-implemented.

### S7. A flat environment cannot have a diamond, so the check correctly finds nothing

Python, Ruby, Swift, and Gradle install **one version of each package**. A diamond finding claims two
dependents forced two *installed* copies; in a flat environment that cannot happen, so `diamond-dep`
runs and reports nothing. That is the right answer rather than a gap — what such an ecosystem has
instead is one version that some dependent is not getting, which is a resolution failure the package
manager already refuses.

A second consequence: a lockfile for a flat environment names an edge by *distribution*, not by copy.
When a resolution forks — `uv.lock` locking `urllib3` twice for two interpreter ranges — an edge
naming it is genuinely ambiguous, and the Python provider drops it rather than attaching it to an
arbitrary copy (`demo/python`).

### S8. A lockfile is feature- and target-agnostic, so a conflict may be one no build compiles

Found by dogfooding, and it changes how the two resolved-tree checks should be read.

Running tropism on this repository reports 17 findings, every one of them a correct statement about
`Cargo.lock`. But `cargo tree --duplicates` shows only **three** duplicate sets in the graph actually
compiled — `syn`, `hashbrown`, `foldhash`. The rest (`thiserror`, `bitflags`, `fixedbitset`,
`cpufeatures`, `getrandom`, `r-efi`) are reachable only through `ratatui`'s optional `termwiz`
backend, which is never enabled, or through another target platform entirely: `r-efi` is UEFI.

`Cargo.lock` is resolved once for *all* feature combinations and *all* targets, and records no
feature information whatsoever. The same is true of npm's `optionalDependencies` and of platform-
specific entries in a `Gemfile.lock`. Deciding which copies a given build compiles needs the feature
resolution of every dependency's own manifest — files that are not in the repository.

**Cost:** `version-conflict` and `diamond-dep` overstate. A finding is true of the lockfile and may
be irrelevant to every build anyone runs.
**Why it stays:** feature resolution is a resolver step, and the dependencies' manifests are not
present to resolve from.
**Mitigation:** none yet. The honest fix is wording — these checks report what the lockfile resolved,
not what the build compiles. Worth stating in the finding message itself.

---

## Deferred — architecture

### D1. ~~Cross-project cycles are invisible~~ — RESOLVED

Cycle detection now runs at two scopes, and every finding carries `details.scope`:

- **`module`** — within one project, from that project's module graph.
- **`project`** — between projects, from the repo-wide edges the rule engine already collects.

`demo/dotnet` reports both: `Shop.Domain.Orders` ↔ `Shop.Domain.Billing` at module scope, and
`Shop.Domain` ↔ `Shop.Data` at project scope, the latter with evidence from both arms — the
`<ProjectReference>` and the `using`.

Before this, the check reported `ok` while two packages were mutually dependent, which is the
silent-clean failure the rest of the tool is built to avoid.

**Remaining:** the project-scoped graph has projects as nodes, so it says *which packages* form the
cycle but not which modules inside them. Sufficient for the finding; a fully qualified
`project::module` graph would be more precise and is not built.

### D2. ~~Lockfile discovery is same-directory only~~ — RESOLVED, differently than proposed

A workspace member now reports *where* the resolved tree is: "no lockfile in this project; the
resolved tree for this workspace is `Cargo.lock`, and version-conflict and diamond-dep run on the
project that owns it."

**The proposed fix — walk upward and adopt the ancestor's lockfile — was measured and rejected.** A
lockfile resolves the whole workspace at once, so handing it to each member reports one shared
resolution once per member: on this repository, the same 17 `Cargo.lock` findings five times over,
attributing a workspace-wide resolution to crates that do not own it. The registered *cost* was
always the misleading message, and that is what was fixed. The checks still run exactly once, on the
project holding the lockfile, and a test pins that they do not duplicate.

### D3. `scan_root` is echoed as given, so absolute paths reach the JSON

Finding paths are correctly relative, but `"scan_root": "/Users/…"` makes output machine-specific
and breaks principle 5 for anyone passing an absolute path.

**Fix:** normalize to `.` on output, or relativize against the working directory.

### D4. ~~Analysis is single-threaded~~ — RESOLVED; the cache is still unbuilt

Extraction runs on `rayon`. Because `map` over an indexed parallel iterator preserves order and the
file list is already sorted, output stays byte-identical — determinism must not become a function of
core count, and there is a test for it.

**It looked worthless when first measured.** 3,000 files took 19.5 s parallel against 20.0 s
single-threaded, because the real cost was not the parse at all — see D39. With that fixed, the same
run is 0.08 s parallel against 0.30 s serial.

**Still open:** the content-addressable cache in [01-architecture.md](01-architecture.md).

### D5. Manifest line numbers are best-effort

`serde_json` and `toml` discard spans, so `find_key_line` scans for the key textually. A wrong line
is visibly wrong rather than silently misleading, but it is still wrong.

**Fix:** a span-preserving parser (`toml_edit` already does this for TOML).

### D23. ~~No way to exclude sample code~~ — RESOLVED

Resolved by `exclude` in `tropism.toml` ([11-dependency-rules.md](11-dependency-rules.md)). Patterns
are applied before discovery, and every exclusion is disclosed in the report with a match count so a
blind spot is never silent. `tropism analyze . --fail-on error` now exits 0 on this repository, which
unblocks the dogfood gate in [13-build-and-release.md](13-build-and-release.md).

Configured in the ruleset rather than as a CLI flag, because what a repository excludes is a
property of the repository, not of the invocation. A CLI `--exclude` would still be useful for
ad-hoc runs against a repository with no `tropism.toml`; it is not built.

**Remaining:** the CLI still accepts only one path argument, so `tropism analyze crates/a crates/b`
exits 2.

### D24. ~~`tropism check`~~ — RESOLVED; `--check <id>` is still not implemented

`tropism check [FILES...]` is built, with `--staged` and `--since <ref>`. It runs the rules only and
scopes findings to the files it was given, which is the division
[10-js-evaluation.md](10-js-evaluation.md) argues for: the sound checks gate, the advisory ones
inform. `.pre-commit-hooks.yaml` ships alongside it. See
[14-incremental-checking.md](14-incremental-checking.md).

**Still missing:** the `--check <id>` filter, for running one named check. Low value now that the
rules/inferred split is a subcommand rather than a flag, which is what the filter was mostly wanted
for.

**Also still open:** the run is scoped but not parse-incremental — D36.

### D25. The workspace root is a virtual manifest, so the repo is not `cargo install`-able

`cargo install --bins --locked --path .` fails with *"found a virtual manifest … instead of a
package manifest"*. The release workflow is unaffected because it builds `-p tropism`, but it blocks
prek's `language: rust` integration, which installs a hook by running exactly that command on a
clone of the hook repository.

**Cost:** one of three routes to the pre-commit hook is closed. The other two —
`language: system` with a released binary, and `repo: local` with `additional_dependencies` from
crates.io — both work.
**Fix:** make the root an installable package by moving the CLI crate to the repository root, the
layout ripgrep uses. Knock-on effects on `tropism.toml`'s module globs and on the documented crate
layout, so it is a decision rather than a detail.

### D39. ~~Rust import resolution was O(files) per import~~ — RESOLVED

Found by benchmarking D4, not by reading. `module_set` rebuilt a `BTreeSet` of every module in the
project on **every unresolved import**, from both the external and the internal resolution paths.
Quadratic over a project, and invisible below about a thousand files.

| Files | Before | After |
| ----- | ------ | ----- |
| 500   | 0.56 s | 0.02 s |
| 1,000 | 2.20 s | 0.04 s |
| 2,000 | 8.73 s | 0.08 s |
| 3,000 | 19.6 s | 0.11 s |

Fixed by memoizing per project in `ProjectContext::local_modules`, a `OnceLock` the provider fills on
first use. Output is byte-identical before and after, which is the check that mattered: the fix had
to be a pure speed change, not a change of answer.

**Two things worth keeping from how this was found.** It only surfaced because D4 was *measured*
rather than assumed — parallelising the parse moved 19.5 s to 19.5 s, which is what said the parse
was not the cost. And `ProjectContext::known_modules` already carried a doc comment claiming "Rust
uses it to avoid recomputing its module set per import", which was simply not true. A comment
asserting a performance property is not a test of one.

**Not audited:** the other nine providers were not checked for the same shape. Any provider deriving
a set from `source_files` inside `resolve_import` has this bug.

### D26. ~~No `LICENSE` file~~ — RESOLVED

`LICENSE` carries the MIT text, and `Cargo.toml` declares `license = "MIT"`. The declaration was
narrowed from `MIT OR Apache-2.0` rather than adding a second text file — one of the two fixes this
entry proposed.

**Worth knowing about the narrowing.** The Rust convention is dual MIT/Apache-2.0, and the reason is
not tidiness: Apache-2.0 carries an express patent grant that MIT does not. Nothing had been
published when this changed, so no downstream user was relicensed; adding Apache-2.0 back later is
possible but needs every contributor's agreement, which is cheap now and expensive later.

This unblocked two things at once — the first release, and the SignPath Foundation application in
[16-signing.md](16-signing.md), whose eligibility test is an OSI-approved licence and whose lead
time dominates the signing work.

---

## Deferred — rules

### D6. `layers`, `require`, and `transitive` are unimplemented

Specified in [11-dependency-rules.md](11-dependency-rules.md), **rejected at parse time with an
error naming the field** so a ruleset never appears to enforce more than it does. `require` is
negative-shaped and inherits S1's weakness; it should ship capped at Medium confidence.

### D7. Version constraints in package rules are unimplemented

`deny = ["lodash < 4.17.21"]` needs ecosystem-correct comparison, and **no provider implements
`VersionOps`** — all four return `None` from `compare` and `satisfies`. Duplicate detection compares
for equality only, which is why nothing has needed it yet.

### D8. No baseline or ratcheting

Teams adopt these tools on codebases that already violate the rules. Without a baseline of accepted
violations, a full-repository run is a wall of errors and the ruleset gets deleted.

**Downgraded.** Incremental checking ([14-incremental-checking.md](14-incremental-checking.md))
gives ratcheting for free: a run scoped to changed files passes on a repository with two hundred
existing violations as long as the commit does not add a two-hundred-and-first. No state file, no
drift, nothing to regenerate after a refactor. A baseline is still wanted for the whole-repository CI
job, but it is no longer a prerequisite for adoption.

**Designed, not built:** [17-baselines.md](17-baselines.md). The crux is that a baseline holds state
and findings move, so it keys on the finding ID *and* on `(rule, from_module, to_module)` with an
occurrence count — a rename keeps a violation in the same module pair and stays baselined, a move
across a boundary does not. Baselined findings are downgraded and counted, never deleted.

### D9. No sub-ruleset inheritance in a monorepo

Whether `packages/web/tropism.toml` extends or replaces the root ruleset is undecided
([11-dependency-rules.md](11-dependency-rules.md), open question 2).

Note this is *not* the same as workspace boundaries, which are now first-class: `[[workspaces]]` in
the root ruleset draws them, and `tropism workspaces` shows what was inferred. One ruleset still
governs the whole scan root.

### D38. An undeclared same-workspace sibling import is exempted, not reported

An import satisfied only because a workspace sibling publishes the name resolves today through
hoisting and breaks when the package is published or built alone. tropism exempts it from
`missing-dep` and **discloses the exemption** rather than reporting it.

**Why not report it.** The exemption is worth 107 findings across the ten repositories in
[10-js-evaluation.md](10-js-evaluation.md), and turning it into a check risks taking those back on a
codebase where nobody considers it a defect.

**Why it is still open.** That 107 is a measurement; the *correctness* of the exemption is not. The
35-finding hand audit's false-positive taxonomy contains no sibling class, so no one has checked
whether these are genuine false positives or a defect class hiding behind a convenience. The corpus
and the method both still exist.

**Fix, if measured:** an `undeclared-sibling` check at Low confidence, never gating. Until then a
team that wants it enforced has `crosses_workspace` for the across-boundary case.

### D10. `version-conflict` and `diamond-dep` overlap

For npm and Cargo, a duplicated package *is* the resolved outcome of a diamond with incompatible
constraints, so both checks fire on the same packages and differ only in whether the dependents are
named. Two findings for one problem.

**Fix:** merge into one check — "installed N times because these dependents disagreed".

### D11. `dependency-bloat` has no definition

Never implemented; reports `Unavailable` with that reason. Deferred in
[07-open-questions.md](07-open-questions.md) and nothing since has changed the argument.

---

## Deferred — per-language

| #   | Language | Limitation                                                                      | Cost                                                                                                                                                                      |
| --- | -------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D12 | Go       | `replace` directives in `go.mod` are ignored                                    | a redirected or dropped requirement can produce a wrong unused-dep                                                                                                        |
| D13 | Go       | `cycle` can never fire on compilable Go                                         | the compiler already rejects import cycles; the check is dead weight and should report as structurally unavailable rather than clean                                      |
| D14 | JS/TS    | only `package-lock.json` is parsed                                              | yarn and pnpm repos get `Unavailable` for resolved-tree checks; yarn's format is bespoke and pnpm's is YAML, which has no maintained crate ([08-crates.md](08-crates.md)) |
| ~~D15~~ | JS/TS | ~~`tsconfig` `paths` aliases are not read~~ — **RESOLVED**                     | read from `tsconfig.json`/`jsconfig.json` via `resolution_config_files`, including `baseUrl`, longest-prefix precedence, and JSONC comments. `extends` is *not* followed — a base config is often a package in `node_modules`, which tropism must not need installed, so those stay `Unresolved` |
| D16 | ~~JS/TS~~ | ~~`package.json` `workspaces` globs are not parsed~~ — **RESOLVED**            | read by `LanguageProvider::workspace_members`, along with `pnpm-workspace.yaml`, Cargo `members`, `go.work`, Maven `<modules>`, and Gradle `include`. See open question 1. |
| D37 | Java/C#  | Gradle `projectDir` remapping and `.sln` files are not read for workspaces      | a remapped Gradle project contributes its default location; a `.sln` contributes nothing and its projects fall back to language grouping                                   |
| D17 | Rust     | `package = "…"` renames are keyed by the import name                            | the real crate name is not tracked, so lockfile matching would miss a renamed dependency                                                                                  |
| D18 | Rust     | `[workspace.dependencies]` inheritance records the requirement as `"workspace"` | no check needs the version yet; would matter for D7                                                                                                                       |
| D19 | C#       | `Directory.Packages.props` is not read                                          | Central Package Management projects have no versions; names are still correct, so current checks are unaffected                                                           |
| D20 | C#       | `.sln` files are not parsed                                                     | projects are found by `.csproj` discovery instead, which is equivalent in practice                                                                                        |
| D21 | C#       | conditional `ItemGroup`s are not evaluated                                      | every branch is taken, deliberately — the same choice as Rust `cfg` — which can overstate dependencies                                                                    |
| D22 | C#       | `System.*` is treated as framework                                              | in older non-SDK projects some shipped as packages, so a genuinely missing reference can hide                                                                             |
| D27 | Python   | `requirements.txt` `-r other.txt` includes are not followed                     | the included file is analyzed on its own if it is also named `requirements.txt`; otherwise its entries are invisible. Inlining it would put a finding's provenance on the wrong file |
| D28 | Python   | `[build-system] requires` is not read                                           | build backends (`hatchling`, `setuptools`) are not recorded, so they can never be reported unused — which is the safe direction, since they are never imported             |
| D29 | Ruby     | `.gemspec` is not a claimed manifest                                            | a gem whose dependencies live only in its gemspec reads as declaring nothing; `Gemfile` covers the application case, which is the common one                              |
| D30 | Java     | Gradle version catalogs (`libs.versions.toml`) are not read                     | `implementation libs.guava` contributes nothing, so a catalog-based build looks like it declares fewer dependencies than it does                                          |
| D31 | Java     | `<properties>` version placeholders are kept raw                                | `${spring.version}` is recorded as the requirement verbatim; no check needs the value yet                                                                                 |
| D32 | Java     | Maven `<parent>` and multi-module inheritance are not resolved                  | a module inheriting dependencies from its parent POM shows only its own, so an import satisfied by an inherited dependency stays unresolved                               |
| D33 | Swift    | a target with a custom `path:` is not found                                     | the file falls back to its directory as the module — less precise, never wrong                                                                                            |
| D34 | C++      | include-path roots are a fixed list                                             | a project using an unconventional root (`headers/`, `api/`) gets component names that no `#include` matches, so its internal edges are lost                               |
| D35 | C++      | `#include MACRO` and generated headers are invisible                            | a computed include names nothing readable and is skipped                                                                                                                  |
| ~~D36~~ | all  | ~~`tropism check` scopes but does not parse incrementally~~ — **RESOLVED**                         | extraction is now scoped to the changed files; module identity, project roots, manifests and the module→file map are still resolved globally because they are line scans rather than parses. Measured 0.37 s → 0.02 s on 107 files, 0.05 s on 3,000. The cost is `CheckOutcome::suppressed`, which becomes `None` rather than a number — see design/14 |

---

## Not built

The MCP server ([05-interfaces.md](05-interfaces.md)), `--format sarif`, and six of the ten target
languages: Python, Java, C++, Swift, Ruby, and TypeScript as a first-class provider distinct from
JavaScript.

Per [10-js-evaluation.md](10-js-evaluation.md) the MCP server should come **before** more languages:
two slices produced two different failure modes, and a third would produce a third without changing
the product question.

---

## Suggested order

1. **Incremental checking and the pre-commit hook**
   ([14-incremental-checking.md](14-incremental-checking.md)) — the strongest differentiator, and it
   resolves D8 as a side effect. Ships with the release pipeline, since a hook needs a binary.
2. ~~**D24**~~ — done. `tropism check` ships, and the hook is built on it. **D36** is what remains:
   making the run parse-incremental rather than merely scoped.
3. **D2** — lockfile discovery upward, which turns several misleading `Unavailable` reasons into
   real answers.
4. **D10** — merge the two overlapping resolved-tree checks.
5. **The MCP server**, which is where the remaining untested product claim lives.

S1 and S3 are not on this list and never will be. They are reported, not fixed.
