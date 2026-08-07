# Known limitations, condensed for triage

A summary of tropism's own register so you can classify an experience without reading the whole
thing. **The register in the repository is authoritative** — `design/12-known-limitations.md` — and
carries the reasoning, the measurements, and the cost of each entry. Read it before filing anything
that looks like a match here.

The split is the important part. **Structural** limitations are consequences of never invoking a
package manager and never executing the analyzed repository; they get reported honestly rather than
fixed, because anything that resolves them trades away the property that lets tropism run in a
pre-commit hook on a fresh checkout. **Deferred** ones are simply not built.

## Structural — do not file as bugs

| # | Limitation | Why it cannot be fixed |
| --- | --- | --- |
| **S1** | `unused-dep` cannot prove a dependency is unused | Packages are used through channels a hermetic tool cannot see: HTML `<script src>`, config files, framework strings, CLI arguments in scripts. Measured at **63% false positives** across ten real repositories. Every fix needs an installed dependency tree or executing config files. |
| **S2** | A path reference proves usage but never absence | Rust writes `anyhow::Result<T>` with no import, so path references must count as usage — but a bare path is also how local types are named, so one can never prove a dependency is *missing*. |
| **S3** | Resolved-tree checks unavailable in most ecosystems | `version-conflict` and `diamond-dep` need exact versions **and edges**. Only npm and Cargo ship both. `go.sum`, `gradle.lockfile`, `Package.resolved`, and `conan.lock` have no edges; Maven has no lockfile at all. |
| **S4** | Manifests that are programs are read incompletely | `Gemfile`, `build.gradle`, `Package.swift`, `conanfile.py` are code. tropism parses the declarative subset with a grammar and never executes them. Dynamic constructs contribute nothing rather than a guess. |
| **S5** | Import name ≠ package name | `import yaml` is `PyYAML`; `com.google.common` is `com.google.guava:guava`. The authoritative mapping lives in installed package metadata. Structural rules plus a curated table cover most; the residue stays unresolved rather than guessed. |
| **S6** | No licence policy | Needs each dependency's licence metadata, which lives in a registry or an installed tree. |
| **S7** | No diamonds in a flat environment | Python, Ruby, Swift, and Gradle install one version per package. A diamond means two dependents forced two *installed* copies; that cannot happen, so the check correctly finds nothing. |
| **S8** | A lockfile is feature- and target-agnostic | It is resolved once for every feature combination and every platform, and records neither — so it can list copies no build compiles. Deciding otherwise needs each dependency's own manifest, which is not in the repository. |

**S5 is the exception worth filing against.** A missing entry in a mapping table is a small,
verifiable, high-value report: "`import cv2` should resolve to `opencv-python`". The limitation is
structural in general; individual gaps are fixable.

## Deferred — worth filing

Effort, not obstacle. A good report can move these up.

| # | Gap |
| --- | --- |
| **D2** | Lockfile discovery is same-directory only — a lockfile at a workspace root is not found from a member |
| **D3** | `scan_root` is echoed as given, so an absolute path can reach the JSON output |
| **D4** | Analysis is single-threaded and uncached |
| **D5** | Manifest line numbers are best-effort |
| **D6** | `layers`, `require`, `transitive` rule kinds are specified but unimplemented — **rejected by name at parse time**, so a ruleset never appears to enforce more than it does |
| **D7** | Version constraints in package rules unimplemented |
| **D8** | No baseline file (largely obviated: `tropism check <files>` ratchets by scope instead) |
| **D9** | No sub-ruleset inheritance in a monorepo — one `tropism.toml` at the scan root |
| **D16** | npm `workspaces` globs are now read, but a `projectDir`-remapped Gradle project and a `.sln` still fall back to language grouping |
| **D10** | `version-conflict` and `diamond-dep` overlap and can double-report |
| **D11** | `dependency-bloat` has no crisp definition and reports unavailable |
| **D24** | `--check <id>` filter not implemented |
| **D36** | `tropism check` scopes but does not parse incrementally — the whole tree is still parsed |

Per-language gaps are also registered (D12–D22, D27–D35): Go `replace` directives ignored, yarn and
pnpm lockfiles unparsed, tsconfig `paths` aliases unread, Gradle version catalogs unread, Maven
parent POM inheritance unresolved, C++ include-path roots a fixed list, and others. If your issue is
language-specific, look there first — it may already be named.

## Already resolved

Do not report these; they were fixed. Mentioned because older discussion may still reference them.

- **D1** — cross-project cycles are now detected repo-wide
- **D23** — `exclude` globs now keep sample code out of the analysis
- **D24** — `tropism check` exists
- **The repo-wide sibling set** — projects are no longer all siblings of one another. A Rust crate
  can no longer make a JavaScript import look declared, and two separate npm workspaces in one
  repository are two workspaces. Run `tropism workspaces` to see the boundaries and where each was
  read from; every exemption is disclosed in the report.

## The two things that will not be built

Worth knowing before writing a feature request, because both have been decided on evidence rather
than preference:

- **Anything needing an installed dependency tree, a build, or code execution.** This is the
  constraint the tool is built on, and it is what makes the pre-commit hook possible at all.
- **Making `unused-dep` reliable.** It cannot be done offline. The 63% figure is measured across ten
  real repositories after three rounds of mitigation, not estimated.
