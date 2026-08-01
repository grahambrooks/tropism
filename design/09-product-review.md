# 09 — Critical product review

Written after completing the Go vertical slice, 2026-07-31. Everything below is grounded in what
building and running it actually produced, not in what the design documents predicted.

> **Partly superseded — read [the revision at the end](#revised-after-ten-languages--2026-08-01)
> first.** This review is kept as written, because its value is the record of what was believed with
> one language built and because most of it held up. One recommendation did not: "make the MCP server
> the product" set itself a gate, and building the other nine languages failed that gate. The
> surviving thesis is one ruleset enforced at commit time and over the whole repository.

**Verdict: continue, but not on the current thesis.** The six checks are the weakest part of the
product, and the Go slice demonstrated that clearly. What is genuinely defensible is narrower and
was never stated as the goal: a hermetic, uniform, agent-queryable dependency map of a polyglot
repository. Re-scope to that, prove it on TypeScript or Python next, and be ready to stop.

## What was built and measured

|                                |                                                                 |
| ------------------------------ | --------------------------------------------------------------- |
| Source                         | ~4,100 lines Rust across four crates                            |
| Tests                          | 102, clippy and rustfmt clean                                   |
| Real repositories analyzed     | 5 (Cobra, Zerolog, Prometheus, color, httprouter), 853 Go files |
| False positives, final         | 0                                                               |
| False positives, first attempt | a 15-module phantom cycle in Prometheus                         |
| Prometheus (726 `.go` files)   | 1.3 s                                                           |

Three of six checks run for Go. Three cannot.

---

## Risk 1 — the native tool already does this, and does it better

The most important measurement in this review. On the fixture with one unused and one undeclared
dependency planted:

|                                          | tropism | `go mod tidy` |
| ---------------------------------------- | ------- | ------------- |
| Found unused `golang.org/x/sync`         | ✅       | ✅             |
| Found undeclared `github.com/rs/zerolog` | ✅       | ✅             |
| Resolved the correct version to add      | ❌       | ✅ `v1.35.1`   |
| Fixed the manifest                       | ❌       | ✅             |
| Pruned stale `// indirect` entries       | ❌       | ✅             |

`go mod tidy` found everything tropism found, then fixed it. tropism found the same two problems, fixed
nothing, and cannot tell you which version to add because that needs a resolver.

This is not a Go quirk. Every ecosystem has an incumbent: `cargo-machete` and `cargo-udeps`,
`depcheck` and `knip`, `deptry`, `mvn dependency:analyze`. All are free, all are already in the
user's toolchain, and most fix rather than merely report.

**tropism cannot win on detection quality. It must win on some other axis or not compete.**

## Risk 2 — the flagship check is dead on arrival in Go

`CLAUDE.md` lists circular dependencies first. Go's compiler rejects import cycles outright:

```
imports example.com/tangle/billing from order.go: import cycle not allowed
```

So `cycle` can only ever fire on Go code that does not compile. The analyzer is implemented and
tested — against fixtures that `go build` rejects, because no valid Go project can exercise it.

This is language-specific: cycles are real and common in Python, JavaScript/TypeScript, Java, C#,
Ruby, and C++. But Go was chosen as the first language for implementation simplicity, and that
choice hid the product's headline feature behind a compiler that already enforces it. **The build
order optimized for the easiest provider, not for learning the most about the product.**

## Risk 3 — the core constraint disables half the product on three languages

The no-native-package-manager rule ([CLAUDE.md](../CLAUDE.md)) is sound, and it has a price that is
now measurable. `version-conflict`, `diamond-dep`, and `dependency-bloat` all need a *resolved*
tree. For Go there is no way to get one offline: `go.sum` records hashes for the whole module graph
rather than the versions MVS selected, and carries no edges at all.

Three of six checks are therefore permanently unavailable for Go. Maven has no lockfile, and
Gradle/NuGet lockfiles are opt-in and usually absent, so Java and C# will land in the same place.
That is **three of ten target languages where half the advertised product cannot run** — not as a
bug, but as the constraint working as designed.

`CheckStatus` reports this honestly, which is the right engineering answer. It is still a product
problem: a user who runs tropism on a Go repo sees three checks that will never work.

## Risk 4 — the target repositories are already clean

All five real repositories produced **zero findings**. Not because tropism is weak — it correctly found
the planted problems in fixtures — but because well-maintained Go projects run `go mod tidy` in CI
and pre-commit hooks.

The user whose dependencies are dirty is, by definition, the user not running the free native tool
that would clean them. The proposition "install this Rust binary to find what the tool you already
have would have fixed" is a hard sell to exactly the population that needs it least, and irrelevant
to the population that needs it most.

## Risk 5 — per-language correctness is language-lawyer work, and it does not amortize

The Go provider needed **three distinct rules about test files**, each discovered only by running on
real code, each a separate round of false positives:

1. A package's `_test.go` files are excluded when that package is built as a dependency of another
   package's tests. Attributing their imports to the package produced a **15-module phantom cycle**
   in Prometheus.
2. Keying on the `package foo_test` clause instead of the filename shrank it to 6 modules — still
   wrong, because it missed in-package test files.
3. Treating all `_test.go` files as one test module then flagged `package doc_test` importing `doc`
   in Cobra, Zerolog, *and* Prometheus — the designed use of an external test package.

Go was chosen as the *simplest* provider. Every language has an equivalent set: Python namespace
packages and conditional imports, TypeScript path mapping and `import type`, Java reflection and
Spring component scanning, the C++ preprocessor. None of this knowledge transfers between providers.

The estimate in [07-open-questions.md](07-open-questions.md) — three trustworthy languages beat ten
mediocre ones — is correct and, if anything, optimistic. Budget weeks per language, not days, and
assume every one ships a false positive that only a real repository will reveal.

## Risk 6 — the hardest problem is still unvalidated

Go was picked partly because import→package mapping is trivial there: the module path is a literal
prefix. The slice therefore proved the *architecture* while skipping the problem
[03-language-providers.md](03-language-providers.md) identifies as central — that `import yaml`
means `PyYAML`, and no structural rule recovers it.

The curated exception table remains unwritten, unsized, and unvalidated. **The slice de-risked the
plumbing and left the actual hard part untouched.**

---

## What is genuinely valuable

Each of these was verified, not assumed.

**1. It is hermetic. This is the strongest claim.**

With a cold module cache and no network, `go mod tidy` fails outright:

```
go: example.com/shop imports
	github.com/spf13/cobra: module lookup disabled by GOPROXY=off
```

tropism analyzed the same directory in milliseconds. For an agent that has just cloned a repository
into a sandbox with no toolchain and no egress, that is the difference between an answer and an
error. No incumbent has this property, because they all work by resolving.

**2. It never executes the analyzed repository.**

`go mod tidy`, `npm install`, and `mvn` all download and run code. tropism reads files. For analyzing
untrusted or unfamiliar code — precisely the agent case — this is a real security property, not a
nice-to-have.

**3. One contract across a polyglot repository.**

A monorepo with Go, TypeScript, and Python services needs three tools with three output formats,
three exit-code conventions, and three severity vocabularies. tropism offers one JSON schema, one
severity model, one exit-code contract. Nothing else does, and the difficulty of assembling it by
hand is what makes it worth buying.

**4. `CheckStatus` is a genuine differentiator for agent consumption.**

No native tool tells you *which checks did not run and why*. An agent reading `go mod tidy`'s silence
cannot distinguish "clean" from "never ran". tropism says:

> `version-conflict — go.sum records hashes for the whole module graph, not the versions MVS
> selected, and carries no edges; a resolved tree needs the Go resolver`

That is the difference between a tool an agent can reason about and one it must guess about. It cost
almost nothing to build and it should be marketed, not buried.

**5. It is fast enough to be invisible.** 1.3 s for 726 files, 0.01 s for small repositories.

---

## Recommendations

**Re-scope the thesis.** Stop selling "finds dependency problems" — the incumbents win that, for
free, with autofix. Sell "a hermetic, uniform, agent-queryable dependency map of a polyglot repo".
Everything tropism does well points that way, and it is the only framing under which the
no-native-tooling constraint is a feature rather than a handicap.

**Make the MCP server the product, not an afterthought.** The CLI competes with entrenched, better
tools. The MCP server has no competitor. Under the re-scoped thesis it should move ahead of further
languages in the build order.

**But first check that the best MCP question is answerable.** `tropism_package_path` — "why is this
package in my tree?" — is the highest-value query in
[05-interfaces.md](05-interfaces.md) and it needs a resolved tree. For Go, offline, that is
impossible. Validate this against a lockfile-bearing ecosystem before committing to the MCP-first
plan, or the flagship interface ships with its flagship query unavailable.

**Do TypeScript or Python next — not more Go.** Both have real import cycles, and both have
lockfiles that *are* resolved graphs (`package-lock.json`, `poetry.lock`), so all six checks can
actually run for the first time. This is the only way to learn whether the full product works, and
Python additionally forces the import→package mapping problem into the open.

**Cut what does not earn its place.** Drop `cycle` for Go specifically (report it as structurally
unavailable — the compiler enforces it — rather than always returning zero). Keep `dependency-bloat`
deferred.

**Set a kill criterion now, while it is cheap to be honest.** Run the TypeScript or Python slice
against ten real repositories. If it finds nothing that the ecosystem's own tooling would not have
caught, the detection thesis is dead and only the hermetic-map thesis remains — which is a smaller,
different, and much more MCP-shaped product. Decide that on evidence rather than on sunk cost.

## Is it worth continuing?

Yes — with the caveat that the *specified* product is weaker than it looks and the *actual* product
is something narrower that has not yet been built.

The architecture held up under real load: the layering is right, the `LanguageProvider` boundary
absorbed every Go-specific rule without touching an analyzer, and `CheckStatus` proved its worth the
moment a check genuinely could not run. That is a good foundation.

The concern is not whether tropism can be built. It is whether "detects dependency problems" is a
problem anyone has, given that `go mod tidy` and its equivalents are free, already installed, and
fix rather than report. The evidence from five real repositories — zero findings — suggests it is
not. The hermetic, uniform, agent-facing framing survives that evidence. The original framing does
not.

---

# Revised after ten languages — 2026-08-01

This review was written from the Go slice alone. Three of its recommendations were followed and one
was not, because building the rest of the languages answered the gate it set on itself.

## What it got right

**Re-scope away from detection.** The kill criterion was run: ten real JavaScript repositories,
recorded in [10-js-evaluation.md](10-js-evaluation.md). Manifest hygiene measured 63% false
positives. The detection thesis for `unused-dep` is dead, and it stays dead — the check is capped at
Medium confidence, defaulted off, and must never gate CI.

**Do TypeScript or Python next, not more Go.** Done, and both taught what was predicted: npm gave the
first genuinely resolved tree, and Python forced the import→package problem into the open.

**Cut what does not earn its place.** `dependency-bloat` is still deferred, and correctly.

## What it got wrong: "make the MCP server the product"

This review's own gate:

> But first check that the best MCP question is answerable. `tropism_package_path` — "why is this
> package in my tree?" — needs a resolved tree. […] Validate this before committing to the MCP-first
> plan, or the flagship interface ships with its flagship query unavailable.

Ten languages later, that validation has an answer, and it is no.

| Can `tropism_package_path` be answered? | Languages                                     | Count |
| --------------------------------------- | --------------------------------------------- | ----- |
| Yes                                     | JavaScript, TypeScript, Rust, Python, Ruby    | 5     |
| Only when an opt-in file exists         | C#                                            | 1     |
| **Never** — no lockfile edges anywhere  | Go, Java, Swift, C++                          | 4     |

The flagship query is dead in four of ten supported languages and conditional in a fifth. This is
structural (S3), not a gap to close: Maven has no lockfile at all, and `go.sum`, `gradle.lockfile`,
`Package.resolved`, and `conan.lock` record versions with no edges.

And S8 damages it further *where it does work*. Ask "why is `thiserror 1.0.69` in my tree?" of this
repository and the honest answer is "through an optional `ratatui` backend that is never enabled".
A path through a feature nobody compiles is a confidently wrong answer to the question the tool was
chosen for.

Two other things moved under the recommendation while it sat unbuilt:

- **`tropism_rules` was billed as the first capability better as an agent interface than as a CLI** —
  ask before acting, with the team's `reason` attached. But the ruleset is a 111-line TOML file with
  the reasons written in it. An agent does not need a protocol to read that; it needs to open the
  file. Most of that tool's value is already delivered by something that exists.
- **Two of six checks are now known to be unreliable.** An MCP surface that hands an agent 63%-false
  findings is worse than a CLI that does, because the agent acts on them without a human reading
  them first.

## The thesis that survives

Not "a hermetic, uniform, agent-queryable dependency map" — that was the right correction from the
wrong evidence. Narrower and stronger:

> **One ruleset, enforced at commit time and over the whole repository, across ten languages, with no
> build and no install.**

Every piece of evidence points here.

- **Rule violations are sound.** A violation is the *presence* of an import — a fact about a line of
  source, the same soundness class as cycle detection, and the opposite of the absence-proving that
  sank `unused-dep`.
- **The hermetic constraint stops being a handicap and becomes the moat.** This is the reversal worth
  stating plainly, because [14-incremental-checking.md](14-incremental-checking.md) is built on it:
  the property that makes the weakest check unreliable is exactly what makes the strongest one
  deployable. ArchUnit needs a compiled classpath. NDepend needs a built solution.
  `dependency-cruiser` needs `node_modules`. A hook that needs a build gets bypassed; tropism needs a
  directory.
- **No native tool can enforce an architecture it was never told about.** `go mod tidy` finds and
  fixes unused dependencies better than tropism ever will. It cannot know that the api layer must not
  reach into storage, because nobody told it, and there is nowhere to tell it.
- **Cross-language is unmatched.** One hook and one ruleset for a repository with a Go backend, a
  TypeScript frontend, and a .NET service. Nothing else offers this at all — and after ten languages
  it is a claim about behaviour rather than intent.

The two surfaces are one product, not two: `tropism check` at commit time and `tropism analyze` over
the whole repository enforce **the same ruleset**, so what CI blocks is what the hook already
blocked. That symmetry is the thing to protect. A rule that only one of them can evaluate would be
worse than no rule.

## Is it still worth continuing?

Yes, and with a clearer target than at any point before. The original framing is dead, the
MCP-first framing failed its own gate, and what is left is a feature nobody else can ship —
substantially built, needing `tropism check` and a release pipeline to reach anyone.

The honest risk is no longer technical. It is that the differentiator only pays off once someone
installs it, and neither the binary nor the hook entry point exists yet. That is the whole of the
remaining work, and it is the shortest path this project has ever had from "tool that exists" to
"tool in someone's workflow every day".
