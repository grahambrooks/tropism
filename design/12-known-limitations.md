# 12 — Known limitations

Every limitation found while building the four language slices and the rule engine, in one place.
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

### S3. Resolved-tree checks are unavailable for three of ten target languages

`version-conflict` and `diamond-dep` need a resolved graph.

| Ecosystem | Lockfile             | Resolved graph?                                                             |
| --------- | -------------------- | --------------------------------------------------------------------------- |
| npm       | `package-lock.json`  | yes                                                                         |
| Cargo     | `Cargo.lock`         | yes                                                                         |
| Go        | `go.sum`             | **no** — hashes for the whole module graph, no edges, not the MVS selection |
| .NET      | `packages.lock.json` | yes, but opt-in and usually absent                                          |
| Maven     | none                 | **no**                                                                      |

**Cost:** on Go, and on most Java and .NET repositories, two of the advertised checks never run.
**Why it stays:** obtaining the tree means running the resolver.
**Mitigation:** `CheckStatus::Unavailable` with the ecosystem-specific reason, so silence is never
mistaken for a clean result.

### S4. Manifests that are programs are read incompletely

`build.gradle[.kts]`, `Package.swift`, `conanfile.py`, and `vitest.config.mts` are code. tropism parses
the declarative subset and cannot see anything dynamic.

**Why it stays:** principle 1 — tropism never executes the repository it analyzes. This is a security
property for the agent use case, not a convenience.

### S5. Import name ≠ package name, and the gap has no complete answer

`import yaml` means `PyYAML`; `using Xunit` means `xunit`. Structural rules and a curated exception
table cover most of it; the residue stays `Unresolved` rather than guessing.

**Cost:** unresolved imports cap hygiene confidence for the whole project.
**Why it stays:** the authoritative mapping lives in installed package metadata.
**Not yet validated:** Python is the worst case and has not been attempted
([09-product-review.md](09-product-review.md), risk 6).

### S6. License policy is out of scope

`cargo-deny`-style licence checks need each dependency's licence metadata, which lives in the
registry or an installed tree. Stated in [11-dependency-rules.md](11-dependency-rules.md) rather
than half-implemented.

---

## Deferred — architecture

### D1. Cycle detection runs per project, so cross-project cycles are invisible

`demo/dotnet` has `Shop.Domain` referencing `Shop.Data` and back. The `cycle` check does not see it;
a **rule** catches it, because rules already evaluate repo-wide.

**Cost:** the most architecturally significant cycles — those spanning packages in a monorepo — are
missed by the check named after them.
**Fix:** the repo-wide module graph of open question 1 in
[07-open-questions.md](07-open-questions.md), which the rule engine already half-builds.
**Priority: highest in this document.** It is the gap most likely to be mistaken for a clean result.

### D2. Lockfile discovery is same-directory only

A Cargo workspace's `Cargo.lock` and an npm workspace's `package-lock.json` live at the root, but
discovery only pairs a lockfile with a manifest in the *same* directory. So `crates/tropism-core`
reports "no lockfile found" while the workspace root runs the resolved-tree checks for everything.

**Cost:** confusing per-project output, and `Unavailable` reasons that are technically true but
misleading.
**Fix:** walk upward for a lockfile when the project has none, stopping at the scan root.

### D3. `scan_root` is echoed as given, so absolute paths reach the JSON

Finding paths are correctly relative, but `"scan_root": "/Users/…"` makes output machine-specific
and breaks principle 5 for anyone passing an absolute path.

**Fix:** normalize to `.` on output, or relativize against the working directory.

### D4. Analysis is single-threaded and uncached

`rayon` is specified for the parse stage and is not a dependency yet; the content-addressable cache
in [01-architecture.md](01-architecture.md) is unbuilt. Prometheus (726 files) takes 1.3 s, so
nothing is urgent.

**Fix:** parallelise the per-file extraction pass, which is already pure.

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

### D24. `tropism check` and `--check <id>` are specified but not implemented

[05-interfaces.md](05-interfaces.md) specifies a rules-only `tropism check` subcommand and a `--check`
filter. Neither exists; `--help` offers only `--format`, `--fail-on`, `--no-ignore`, `--rules`, and
`--no-rules`.

**Cost:** no fast, high-signal subset for a pre-commit hook, and no way to gate on rules alone while
leaving the noisier checks advisory — which is the division
[10-js-evaluation.md](10-js-evaluation.md) argues for.

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
violations, the first run is a wall of errors and the ruleset gets deleted.

**Priority: high.** This is what makes adoption possible at all, and it should probably ship before
any further rule kinds.

### D9. No sub-ruleset inheritance in a monorepo

Whether `packages/web/tropism.toml` extends or replaces the root ruleset is undecided
([11-dependency-rules.md](11-dependency-rules.md), open question 2).

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
| D15 | JS/TS    | `tsconfig` `paths` aliases are not read                                         | `@/components/Button` stays `Unresolved`, lowering the resolution rate                                                                                                    |
| D16 | JS/TS    | `package.json` `workspaces` globs are not parsed                                | siblings are inferred from discovered projects, which works but is indirect                                                                                               |
| D17 | Rust     | `package = "…"` renames are keyed by the import name                            | the real crate name is not tracked, so lockfile matching would miss a renamed dependency                                                                                  |
| D18 | Rust     | `[workspace.dependencies]` inheritance records the requirement as `"workspace"` | no check needs the version yet; would matter for D7                                                                                                                       |
| D19 | C#       | `Directory.Packages.props` is not read                                          | Central Package Management projects have no versions; names are still correct, so current checks are unaffected                                                           |
| D20 | C#       | `.sln` files are not parsed                                                     | projects are found by `.csproj` discovery instead, which is equivalent in practice                                                                                        |
| D21 | C#       | conditional `ItemGroup`s are not evaluated                                      | every branch is taken, deliberately — the same choice as Rust `cfg` — which can overstate dependencies                                                                    |
| D22 | C#       | `System.*` is treated as framework                                              | in older non-SDK projects some shipped as packages, so a genuinely missing reference can hide                                                                             |

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

1. **D1** — the repo-wide module graph. Fixes the gap most likely to be read as a clean result, and
   the rule engine already builds half of it.
2. **D8** — baseline/ratcheting, without which no team can adopt the rules on an existing codebase.
3. **D2** — lockfile discovery upward, which turns several misleading `Unavailable` reasons into
   real answers.
4. **D10** — merge the two overlapping resolved-tree checks.
5. **The MCP server**, which is where the remaining untested product claim lives.

S1 and S3 are not on this list and never will be. They are reported, not fixed.
