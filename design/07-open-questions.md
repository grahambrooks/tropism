# 07 — Open questions

Decisions deliberately not made in the other documents, with what each one blocks. These need an
answer from the project owner rather than a default chosen by whoever implements first.

## 1. Monorepo and workspace semantics

[01-architecture.md](01-architecture.md) analyzes each project root independently. That leaves a real
case unhandled: in a monorepo, package A imports package B from the same repo. Is that an internal
module edge, an external dependency, or both?

It matters because it changes the answer to every check. If cross-project imports are external, a
monorepo with undeclared internal imports lights up with false "missing dependency" findings. If they
are internal, cycles *between* packages become detectable — arguably the most valuable finding gdep
could produce for a monorepo.

**Recommendation:** treat sibling projects as internal for missing-dep purposes and build a
package-level graph so inter-package cycles are reported. Confirm before building, because retrofitting
a repo-wide graph onto a per-root pipeline is expensive.

**Update — this is now forced.** The JS slice implemented the `missing-dep` half (workspace siblings
and root-hoisted dependencies are visible to children; see
[10-js-evaluation.md](10-js-evaluation.md)). The graph half is settled by
[11-dependency-rules.md](11-dependency-rules.md): a rule like "the CLI must not depend on the MCP
server" spans two projects, so a repo-wide module graph is a precondition of the ruleset, not an
optional refinement.

*Blocks:* graph construction, missing-dep analyzer, and the Rust/JS providers where workspaces are
the norm.

## 2. Default granularity for cycle detection

File-level cycles are numerous and frequently benign; package-level cycles are rarer and more
meaningful. [04-analyzers.md](04-analyzers.md) computes both. Which is on by default?

Defaulting to file-level risks a wall of findings on first run — the fastest way to get a tool
switched off. Defaulting to package-level risks looking like it found nothing.

**Recommendation:** package/directory-level cycles as Warning by default, file-level available behind
a flag.

*Blocks:* nothing structurally; it is a defaults decision that can be made late, but it shapes the
first impression of the tool.

## 3. Does dependency bloat ship in v1?

It is the only check with no crisp definition ([04-analyzers.md](04-analyzers.md)), it needs a
curated equivalence table, and every finding is Low confidence. It is also the check most likely to
generate arguments rather than fixes.

**Recommendation:** defer. Ship the five well-defined checks, and add bloat once there is real usage
to calibrate the thresholds against. It is listed in [CLAUDE.md](../CLAUDE.md) as in-scope, so this
is explicitly a request to cut it from the first version, not to drop it.

*Blocks:* nothing — deferring is the cheap option.

## 4. Diamond dependencies: reported or queried?

Plain diamonds are near-universal and mostly uninteresting. [04-analyzers.md](04-analyzers.md)
proposes reporting only consequential ones and treating the rest as Info.

**Recommendation:** consequential diamonds in the default finding set; plain diamond enumeration
available only through `gdep_package_path` and `gdep graph`. Confirm this is the intended reading of
"diamond dependencies" as a headline feature.

*Blocks:* the diamond analyzer's output shape.

## 5. Which languages ship first?

Ten providers is a lot of surface for a first release, and [06-testing.md](06-testing.md) argues that
three trustworthy languages beat ten mediocre ones.

**Recommendation:** Go first (simplest resolution, proves the trait shape), then Rust (workspace
handling, and it is the language gdep is written in so dogfooding is free), then Python (hardest
resolution — if the design survives it, the design is right). JS/TS fourth, since three lockfile
formats make it the largest single chunk of work. Java, C#, C++, Swift, Ruby after.

*Blocks:* sequencing only, but it determines what "done" means for a first release.

## 6. Is a no-lockfile repository a first-class case?

Maven has no lockfile; Gradle and NuGet lockfiles are opt-in and usually absent
([CLAUDE.md](../CLAUDE.md)). For a large share of Java and C# repositories, three of six checks will
permanently report `Unavailable`.

Is that acceptable, or should gdep do something more for those ecosystems? The alternatives all have
costs: reading the local package cache (`~/.m2`) makes results machine-dependent; approximating
resolution means reimplementing a resolver; fetching metadata breaks the offline property.

**Recommendation:** accept it. Report clearly, do not compromise the no-execution or offline
properties. But this should be a conscious decision, because it caps gdep's usefulness on Java.

*Blocks:* whether Java is worth prioritizing at all.

## 7. Version representation

[02-data-model.md](02-data-model.md) flags that SemVer, PEP 440, Maven ordering, and RubyGems differ.
Opaque versions with per-provider comparison, or an enum of per-ecosystem types?

**Recommendation:** opaque plus `VersionOps`, since it keeps `gdep-core` free of ecosystem knowledge.
The cost is that core cannot sort versions without a provider in hand.

*Blocks:* the core data model — this is the earliest of these decisions to be needed and the most
expensive to reverse.

## 8. Distribution of the exception table

The import→package exception table ([03-language-providers.md](03-language-providers.md)) will need
updating far more often than the binary. Embedded at compile time, loaded from a data file at
runtime, or updatable from a URL?

The last option conflicts with the offline property, so it would have to be explicitly opt-in.

**Recommendation:** embedded, with a config-file override so a user can add entries locally without
waiting for a release.

*Blocks:* provider implementation, but only lightly.

## 9. MCP transport

stdio only, or stdio plus HTTP? stdio covers local agent use, which is the stated purpose. HTTP would
allow a shared analysis server for a team but introduces auth and lifecycle concerns that are
entirely absent from stdio.

**Recommendation:** stdio only until someone asks.

*Blocks:* nothing early.

---

## Proposed build order

Not an open question so much as a proposal to confirm. Each step is independently useful, which
means the project has something worth running from early on.

1. **Skeleton** — workspace, `Report` types, JSON serialization, CLI that outputs an empty report.
   Fixes the contract in [05-interfaces.md](05-interfaces.md) before anything depends on it.
2. **Go provider, imports only** — discovery, `go.mod` parsing, tree-sitter import extraction.
3. **Cycle analyzer** — the first genuinely useful output, and it validates the module graph.
4. **Manifest hygiene** — unused and missing, with the resolution-rate tracking that keeps them
   honest.
5. **Lockfile parsing + version conflicts** — the first check requiring a resolved tree.
6. **MCP server** — over an analysis core that is already proven through the CLI.
7. **Second and third languages** — Rust, then Python. Whatever forces a change to the
   `LanguageProvider` trait here is the real lesson of the whole design.

**Revised after two slices and the rules specification.** Steps 1–5 are done for Go and
JavaScript/TypeScript, and the second language duly forced two trait changes (`resolve_import` needs
the importing file; `ProjectContext` needs the project's own file list). The order from here is:

1. **The repo-wide module graph**, which rules require and open question 1 above deferred.
2. **The ruleset** ([11-dependency-rules.md](11-dependency-rules.md)) — the first feature whose value
   does not depend on out-competing an incumbent.
3. **The MCP server**, now with `gdep_rules` as its highest-value tool.
4. **More languages**, last. Two slices produced two different failure modes; a third would produce
   a third, and none of them changes the product question.

The trait will be wrong the first time. Adding the second language is what corrects it, which is why
it comes before the remaining eight rather than after two more Go features.
