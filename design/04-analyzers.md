# 04 — Analyzers

Each analyzer is a pure function from graphs to findings, with a declared precondition. If the
precondition is unmet the analyzer does not run and the report records `Unavailable` with the reason
— see [02-data-model.md](02-data-model.md).

```rust
trait Analyzer {
    fn check_id(&self) -> CheckId;
    fn requires(&self) -> Requirements;   // ModuleGraph | ResolvedTree | Both
    fn run(&self, ctx: &AnalysisContext) -> Vec<Finding>;
}
```

Summary of what each check needs — this table is the reason the three-way grouping in
[CLAUDE.md](../CLAUDE.md) exists:

| Check             | Module graph | Manifest | Lockfile | Confidence ceiling |
| ----------------- | ------------ | -------- | -------- | ------------------ |
| Circular deps     | ✅            | —        | —        | High               |
| Unused deps       | ✅            | ✅        | —        | Medium             |
| Missing deps      | ✅            | ✅        | —        | Medium             |
| Version conflicts | —            | —        | ✅        | High               |
| Diamond deps      | —            | —        | ✅        | High               |
| Dependency bloat  | —            | ✅        | ✅        | Low                |

Cycles and hygiene work on any checkout. The bottom three need a lockfile and are simply unavailable
without one.

---

## Circular dependencies

**Input:** module graph. **Algorithm:** Tarjan's strongly connected components. Any SCC with more
than one node is a cycle; self-loops are reported separately or ignored.

Report **one finding per SCC, not one per cycle.** A tangle of 12 mutually-importing modules
contains an enormous number of distinct cycles and exactly one problem. Emit the SCC membership plus
one representative shortest cycle as an illustration.

Details to get right:

- **Granularity is a real decision.** File-level cycles are common and often benign (especially in
  languages with no file-level import restrictions); package/directory-level cycles are the ones
  that indicate architectural trouble. Compute at file level, and aggregate to directory level as a
  separate severity tier.
- Severity should scale with SCC size and with whether the cycle crosses a package boundary.
- Rank findings by SCC size so the worst tangle is first.

Cycles are the highest-confidence check gdep has: derived from explicit imports, no name resolution
against a manifest required, no lockfile needed. **Build this first** — it proves the module graph
end to end and is useful on its own.

## Unused dependencies

**Input:** module graph + manifest. Declared dependencies with no import resolving to them.

This check generates false positives more readily than any other, and each cause needs explicit
handling rather than being left to surprise the user:

- **`DepKind` scoping.** A dev-dependency is used if imported anywhere in test/bench/example sources.
  Compare each kind against the right source set; never compare dev-deps against `src/` only.
- **Non-import usage.** Build tools, plugins, linters, type stubs, and CLI-only packages are
  legitimately never imported. Maintain a per-ecosystem allowlist of known tool packages, and treat
  anything declared in a plugin/tool section of the manifest as used.
- **Transitively required but directly declared** — pinning a transitive dependency for a security
  fix is deliberate. If a package is declared *and* appears in the resolved tree as someone else's
  dependency, downgrade or suppress.
- **Dynamic usage.** If the project contains dynamic imports (per
  [03-language-providers.md](03-language-providers.md)), cap this check at Low confidence project-wide.
- **Low resolution rate.** If many imports were `Unresolved`, unused findings are unreliable —
  downgrade and say why.

Given all of that: default this check to Warning, never Error, and make the false-positive escape
hatch (per-dependency suppression) a first-class feature rather than an afterthought.

## Missing dependencies

**Input:** module graph + manifest. Imports that resolve to `External` but match no declared
dependency.

Preconditions that must hold before a finding is emitted, or this check becomes noise:

- The import is not stdlib (`is_stdlib`).
- The import is not `Internal` — relative imports and in-project modules are excluded.
- The import is not provided by another project root in the same monorepo/workspace.
- The import name actually resolved; `Unresolved` never produces a missing finding.

The genuinely interesting case is an import satisfied only *transitively* — it works today because
some other dependency happens to pull it in, and breaks silently when that dependency changes. Where
a resolved tree is available, distinguish "not available at all" (Error) from "available only
transitively" (Warning). Without a lockfile, only the weaker form is detectable.

## Version conflicts

**Input:** resolved tree. The same package resolved at more than one version.

What "conflict" means is ecosystem-specific, and applying one definition everywhere would be wrong:

- **npm** nests by design; multiple versions coexisting is normal, not a defect. Report as Info
  unless duplication is large, and flag hard conflicts only for singleton-required packages (peer
  dependencies).
- **Cargo** allows multiple semver-incompatible majors to coexist; two `1.x` versions cannot happen,
  two of `0.x`/`1.x`/`2.x` can and is worth reporting as bloat rather than breakage.
- **Maven/Gradle** flatten to one version per artifact — the *loser* of that resolution is the real
  finding, because code compiled against the other version may break at runtime. Highest severity
  here.
- **Python** has a single flat environment; two versions is genuinely impossible, so the finding is
  instead *unsatisfiable requirements* across manifests.

Use `version_ops()` for comparison. Do not compare version strings lexically.

## Diamond dependencies

**Input:** resolved tree. A package reachable from the root by two or more distinct paths.

Raw diamonds are extremely common and mostly harmless — reporting every one is noise that will get
the tool ignored. Report only where the diamond has consequences:

- The paths demand *different, incompatible* version ranges (a diamond that forces resolution).
- Resolution had to pick a version that satisfies one dependent but not the other's stated range.

Emit the two (or more) paths as evidence — a diamond finding with no paths shown is unactionable.
Otherwise treat plain diamonds as Info at most, and consider making them query-only (available via
MCP on request) rather than part of the default finding set.

## Dependency bloat

**Input:** manifest + resolved tree. The weakest-defined check, and the one most likely to be
subjective — it must not ship without a concrete definition, or it becomes an opinion generator.

Start with signals that are individually defensible and cite the evidence for each:

- **Transitive fan-out**: a direct dependency that alone pulls in a large subtree.
- **Single-use dependencies**: a package imported at exactly one site, especially where the subtree
  it drags in is large.
- **Duplicated capability**: multiple packages from a known-equivalent set (three date libraries,
  two HTTP clients) — requires a curated data table, same shape as the exception table in
  [03-language-providers.md](03-language-providers.md).
- **Unmaintained or superseded** packages — requires external data gdep does not have offline. Out
  of scope for now; note it as a boundary rather than half-implementing it.

Every bloat finding is Low confidence and Info severity by default. It is advice, and it should read
as advice.

---

## Cross-cutting

**Suppression.** A config file at the project root listing suppressed finding IDs or
`(check, target)` pairs, with an optional reason field. Needed by every check that can be wrong,
which is most of them. Design it in before the first false positive arrives, not after.

**Severity is configurable.** Defaults above are defaults. CI users need to promote a check to Error
to gate merges, and to demote checks they have accepted.

**Analyzers never do I/O.** They receive a fully-built `AnalysisContext`. This makes every one of
them testable against a hand-constructed graph with no fixture repository at all — see
[06-testing.md](06-testing.md).
