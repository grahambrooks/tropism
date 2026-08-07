# 07 — Open questions

Decisions deliberately not made in the other documents, with what each one blocks. These need an
answer from the project owner rather than a default chosen by whoever implements first.

## 1. Monorepo and workspace semantics

[01-architecture.md](01-architecture.md) analyzes each project root independently. That leaves a real
case unhandled: in a monorepo, package A imports package B from the same repo. Is that an internal
module edge, an external dependency, or both?

It matters because it changes the answer to every check. If cross-project imports are external, a
monorepo with undeclared internal imports lights up with false "missing dependency" findings. If they
are internal, cycles *between* packages become detectable — arguably the most valuable finding tropism
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

### RESOLVED — but the two halves were not symmetric, and that was the real question

Both halves shipped, and measuring them against each other showed they disagreed. The graph half is
repo-wide *by design* and correct. The hygiene half was repo-wide *by accident*: the sibling set was
every project in the scan root, unfiltered by language or workspace. Two consequences, both
reproduced before the fix and both now regression-tested in
`crates/tropism-lang/tests/workspaces.rs`:

- A JavaScript project importing `mylib` was exempted from `missing-dep` because a **Rust crate** in
  the repository published that name.
- A package in one npm workspace importing a package published in a **separate** npm workspace was
  exempted, though npm would fail to resolve it.

In both cases the rule engine reported the very same edge as a violation. One analysis, two answers
about one import — and the hygiene answer was the silent one, which is the failure mode the rest of
the tool is organized against.

**The question was never "internal or external".** It is *what bounds the set of mutually-importable
projects*, and the answer is now in [`crates/tropism-core/src/workspace.rs`](../crates/tropism-core/src/workspace.rs),
established three ways, most authoritative first:

| Origin | Source | Ecosystems |
| ------------ | ---------------------------------------- | ---------------------------------------------------------- |
| `configured` | `[[workspaces]]` in `tropism.toml` | any |
| `declared` | the ecosystem's own file | Cargo `members`, npm `workspaces`, `pnpm-workspace.yaml`, `go.work`, Maven `<modules>`, Gradle `include` |
| `language` | inferred: everything unclaimed, by language | Python, Ruby, Swift, C++, NuGet — which declare nothing |

Four decisions worth not re-litigating:

- **Language is checked in addition to membership, never instead of it.** No workspace declaration,
  however authoritative, makes an `.rlib` importable from Node. This is why the fallback is a
  narrowing of the old behaviour and never a widening — it cannot introduce a false positive that
  the unbounded set did not already have.
- **Ancestor hoisting is bounded by the directory tree, not by the workspace.** Node's resolution
  walks up parent directories whether or not a workspace is involved, so requiring workspace
  membership there would have been wrong for the very case that motivated it.
- **The exemption is disclosed, not silent.** `ProjectReport::sibling_exemptions` carries the
  package, the project that supplied it, and the import count, for the same reason `exclude` reports
  a match count. It is also the honest form of a judgement call: an undeclared sibling import works
  today through hoisting and breaks on publish, so a reader deserves to know it happened even while
  tropism declines to call it an error.
- **Whether a crossing is an error is the team's call**, expressed as `crosses_workspace = true` — a
  rule, at High confidence, rather than another inferred check whose severity tropism would have to
  guess. This is the same argument that made rules the product.

**Still open, deliberately:** whether an undeclared *same-workspace* sibling import should become a
finding of its own (an `undeclared-sibling` check). The exemption's value is measured —
[10-js-evaluation.md](10-js-evaluation.md) shows it removing 107 findings across ten real
repositories — but its *correctness* was asserted rather than audited: the 35-finding hand audit's
taxonomy contains no sibling class. The ten-repo corpus still exists, so this is testable rather
than arguable. It should be measured before it is built.

*Blocked nothing further.* `tropism workspaces` prints the boundaries and the crossings;
`tropism explain <file>` says how each import in a file was classified and why.

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
available only through `tropism_package_path` and `tropism graph`. Confirm this is the intended reading of
"diamond dependencies" as a headline feature.

*Blocks:* the diamond analyzer's output shape.

## 5. Which languages ship first?

Ten providers is a lot of surface for a first release, and [06-testing.md](06-testing.md) argues that
three trustworthy languages beat ten mediocre ones.

**Recommendation:** Go first (simplest resolution, proves the trait shape), then Rust (workspace
handling, and it is the language tropism is written in so dogfooding is free), then Python (hardest
resolution — if the design survives it, the design is right). JS/TS fourth, since three lockfile
formats make it the largest single chunk of work. Java, C#, C++, Swift, Ruby after.

*Blocks:* sequencing only, but it determines what "done" means for a first release.

## 6. Is a no-lockfile repository a first-class case?

Maven has no lockfile; Gradle and NuGet lockfiles are opt-in and usually absent
([CLAUDE.md](../CLAUDE.md)). For a large share of Java and C# repositories, three of six checks will
permanently report `Unavailable`.

Is that acceptable, or should tropism do something more for those ecosystems? The alternatives all have
costs: reading the local package cache (`~/.m2`) makes results machine-dependent; approximating
resolution means reimplementing a resolver; fetching metadata breaks the offline property.

**Recommendation:** accept it. Report clearly, do not compromise the no-execution or offline
properties. But this should be a conscious decision, because it caps tropism's usefulness on Java.

*Blocks:* whether Java is worth prioritizing at all.

## 7. Version representation

[02-data-model.md](02-data-model.md) flags that SemVer, PEP 440, Maven ordering, and RubyGems differ.
Opaque versions with per-provider comparison, or an enum of per-ecosystem types?

**Recommendation:** opaque plus `VersionOps`, since it keeps `tropism-core` free of ecosystem knowledge.
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
3. **The MCP server**, now with `tropism_rules` as its highest-value tool.
4. **More languages**, last. Two slices produced two different failure modes; a third would produce
   a third, and none of them changes the product question.

The trait will be wrong the first time. Adding the second language is what corrects it, which is why
it comes before the remaining eight rather than after two more Go features.

**Revised again, after all ten languages.** Steps 1 and 2 of the previous revision are done: the
repo-wide graph landed with project-scoped cycles, and the ruleset is enforced on this repository.
Step 4 — "more languages, last" — is now also done, and it answered its own question. The first four
languages forced four trait changes; the last five forced **none**. `LanguageProvider` converged.

What the five did produce is a sharper map of where the *ecosystems* differ, which is worth more than
another trait method:

- Only Cargo and npm ship a resolved tree with edges. Four of the five new ecosystems ship a flat
  list, and Maven ships nothing.
- Four of the ten manifests are programs. All four are now parsed with a grammar, and two of them
  reuse a grammar already in the tree (`conanfile.py` → Python, `Gemfile` → Ruby).
- Swift is the only ecosystem that states the import→package mapping itself, which is the one clean
  answer to S5 anywhere in the ten.

**Revised once more, and this time the order changes.** The previous revision put the MCP server
next, on the strength of [09-product-review.md](09-product-review.md)'s "make the MCP server the
product". That review set a gate on its own recommendation — validate that
`tropism_package_path` is answerable before committing — and building the remaining five languages
answered it: **no**. The query needs a resolved tree with edges, and four of the ten ecosystems have
none at all. Details in [09-product-review.md](09-product-review.md), "Revised after ten languages".

The thesis that survives all the evidence is narrower and better:

> **One ruleset, enforced at commit time and over the whole repository, across ten languages, with
> no build and no install.**

Everything tropism does well points there, and it is the only claim with no competitor. The order
follows from it:

1. **`tropism check [FILES...]`** — [14-incremental-checking.md](14-incremental-checking.md). The
   file-list form first; `--staged` and `--since` are sugar over it and can be dropped without loss.
   This is the whole differentiator: sound checks, scoped to a change, in well under a second,
   needing nothing installed. It also gives ratcheting on an already-violating codebase for free,
   which is what makes the rules feature adoptable outside greenfield.
2. **The release pipeline** — [13-build-and-release.md](13-build-and-release.md). Both a hook and an
   MCP server need an installable binary; nothing reaches a user without it. Note the ordering
   constraint in D25: `language: rust` in a hook framework wants the repository root to be an
   installable package.
3. **A baseline for whole-repository runs** — D8, designed in [17-baselines.md](17-baselines.md).
   Once `check` exists the ratchet covers the commit path, and the gap left is the CI job that checks
   everything. That is where a baseline earns its place, and not before.
4. **The unimplemented rule kinds** — `layers`, `require`, `transitive`, and version constraints,
   all currently rejected at parse time rather than silently ignored.
5. **MCP, scoped down** — three tools rather than seven. Last, not first: see
   [05-interfaces.md](05-interfaces.md).

What moved *out* of the plan: more languages (all ten are built), and MCP-first (the gate failed).
