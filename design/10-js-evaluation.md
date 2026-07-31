# 10 — JavaScript/TypeScript evaluation

The kill-criterion run set in [09-product-review.md](09-product-review.md): build a second slice
where all six checks *can* run, point it at ten real repositories, and decide on evidence whether
the detection thesis survives.

**Result: the detection thesis does not survive for manifest hygiene, and does survive for cycles.**
That split is sharper than expected and it should drive the roadmap.

## Why JavaScript was the right test

Go answered nothing, because `go mod tidy` already does the job and Go's compiler forbids cycles.
JS/TS was chosen to remove all three of Go's excuses:

|                              | Go                        | JavaScript/TypeScript        |
| ---------------------------- | ------------------------- | ---------------------------- |
| Built-in tidy equivalent     | `go mod tidy`             | **none** — npm never prunes  |
| Import cycles possible       | no, compiler rejects them | **yes**, legal and common    |
| Lockfile is a resolved graph | no, `go.sum` is hashes    | **yes**, `package-lock.json` |

So a null result here could not be blamed on the ecosystem.

## What was measured

Ten repositories: axios, chalk, date-fns, express, got, lodash, react-router, swr, TanStack Query,
zustand. Roughly 4,200 source files. Whole-set runtime under 4 s; the largest repo, 1,627 files, in
0.75 s.

**First run: 1,171 findings.** Sampling showed they were overwhelmingly wrong. Three rounds of
mitigation followed, each aimed at a false-positive class my own design had predicted and I had not
built:

| Mitigation                                                        | Findings |
| ----------------------------------------------------------------- | -------- |
| — (first run)                                                     | 1,171    |
| Tooling `DepKind`: `@types/*` and packages invoked from `scripts` | 732      |
| Workspace siblings exempt from `missing-dep`                      | 625      |
| Root-hoisted `devDependencies` visible to workspace children      | **625**  |

`missing-dep` went from noise to nearly clean — 96 findings to 4 on TanStack Query. `unused-dep` did
not: 245 remain on that one repository.

## The audit

Volume proves nothing. Every `unused-dep` and `missing-dep` finding across the five smallest
repositories — 35 in total — was checked against the source by hand.

| Verdict                                                        | Count  |         |
| -------------------------------------------------------------- | ------ | ------- |
| True positive — package referenced nowhere                     | 13     | 37%     |
| **False positive — package genuinely used, invisibly to tropism** | **22** | **63%** |

The false positives are not sloppiness. Each is a real use through a channel tropism structurally
cannot see:

- **HTML `<script src>` tags** — lodash loads `qunitjs`, `platform`, `requirejs`, `dojo`, and
  `benchmark` from `test/underscore.html` as `src="../node_modules/platform/platform.js"`. Seven of
  the 22.
- **Config-file references** — `environment: 'jsdom'` in a vitest config, `extends:
  "@sindresorhus/tsconfig"` in tsconfig, `@vitest/ui` in a GitHub Actions workflow. Six.
- **Framework string loading** — express selects a view engine with `exports.engine = 'hbs'`. The
  package name is a string in application code.
- **Command-line argument strings** — got uses `tsx` via `'--import=tsx/esm'` passed to a spawned
  process.
- **Not-shipped code** — `examples/` directories importing packages the library itself does not
  depend on.

## Why this does not have a fix

Each remaining class needs a different parser for a different file format with a different schema:
HTML, `tsconfig.json`, `vitest.config.mts` (a *program*), `.eslintrc`, GitHub Actions YAML, and
arbitrary shell strings. There is no general rule, only an unbounded tail of special cases — and
`vitest.config.mts` cannot be read without executing it, which principle 1 forbids.

The deepest instance is structural. `gzip-size-cli` provides the `gzip-size` binary,
`npm-run-all2` provides `run-p`, `typescript` provides `tsc`. Mapping a script command back to the
package that provides it requires reading `bin` fields from `node_modules/*/package.json` — which
means requiring an installed tree, which is exactly the hermetic property that is tropism's strongest
differentiator.

**The constraint that makes tropism valuable is the same constraint that makes `unused-dep`
unreliable.** That is not a bug to be fixed. It is the shape of the problem.

## What did hold up

**Cycle detection.** Real cycles were found in got (7 modules), react-router (5 findings), date-fns
and TanStack Query (3 each), and swr. Spot-checked by hand: got's is a genuine 7-module tangle
across `source/core/`, with accurate files and line numbers.

This check is everything manifest hygiene is not:

- **Sound by construction.** It reads only import statements, which are unambiguous syntax. There is
  no equivalent of "used via an HTML script tag" — an import either exists in the source or it does
  not, so the false-positive class that sinks `unused-dep` cannot arise.
- **Needs no installed tree.** Fully hermetic, so tropism's core constraint costs nothing here.
- **Genuinely painful in JS/TS.** Import cycles cause temporal-dead-zone crashes and undefined
  imports at module-init time, and they are legal, so nothing in the toolchain stops them.
- **File-level, which is where JS cycles actually live.** Go's directory-level graph would miss
  every one of these; the provider-controlled module mapping made this a one-line difference.

**The resolved-tree checks ran for the first time.** `version-conflict` found 39 duplicated packages
in axios and `diamond-dep` attributed 30 of them to the dependents that disagreed. Both are correct
and neither is available offline in Go.

They also revealed a design error: for npm, **`version-conflict` and `diamond-dep` are nearly the
same check.** A duplicated package *is* the resolved outcome of a diamond with incompatible
constraints, so the two fire on the same packages and differ only in whether the dependents are
named. `design/04-analyzers.md` treated them as independent. One should absorb the other.

## Recommendations

**Make cycle detection the product.** It is the only check that is sound under the hermetic
constraint, it is the one with no built-in equivalent in the ecosystems where cycles are legal, and
it is the one whose findings were all real. The competitor is `madge` — one language, no MCP, no
uniform contract.

**Demote manifest hygiene to opt-in, and label it.** At a 63% false-positive rate it must not be on
by default and must never gate CI. If kept, cap it at `Confidence::Low` for JS/TS and say plainly in
the message that config-file and HTML references are invisible. The honest alternative is to drop it
for JS/TS entirely and cede that ground to `knip`, which reads the config files tropism will not.

**Merge `diamond-dep` into `version-conflict`.** One check: "this package is installed N times
because these dependents disagreed." Two checks reporting the same packages is noise.

**Do not add languages next.** Two slices produced two different failure modes — Go said nothing,
JS said too much — and both trace to the same root: what counts as "used" is ecosystem-specific and
often invisible without an installed tree. A third language will produce a third failure mode. The
question worth answering now is not "does this work for Python?" but "is the cycle-plus-hermetic-map
product worth building?", and that is answered by building the MCP server over what already works.

**Revised kill criterion, for the MCP step.** Ship `tropism_summary`, `tropism_findings`, and
`tropism_package_path` over the Go and JS slices, and drive a real agent task with them. If the agent
cannot do something it could not do with `grep` and `madge`, stop. The remaining value is
concentrated in the uniform, hermetic, agent-facing interface, and that claim is now the only one
still untested.

## Verdict update

[09-product-review.md](09-product-review.md) concluded "continue, but not on the current thesis."
This evaluation sharpens that:

- **Cycle detection: keep.** Sound, hermetic, useful, and real on every repo that had one.
- **Resolved-tree checks: keep, merged.** Correct where a real lockfile exists; unavailable in three
  of ten target languages, and honest about it.
- **Manifest hygiene: demote or drop.** 63% false positives after three rounds of mitigation, with
  the residual causes unfixable inside the project's core constraint.
- **`dependency-bloat`: stays deferred.** Nothing here changes that.

The product is smaller than specified and more defensible than it was. Half of what
[CLAUDE.md](../CLAUDE.md) advertises should not ship on by default.
