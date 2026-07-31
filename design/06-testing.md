# 06 — Testing

The hard part is that gdep has **no oracle**. For most repositories nobody knows the true set of
unused dependencies, so "run it on a big repo and see" validates nothing. The strategy is to test
each layer where its correct answer *is* knowable.

## Layer by layer

**Analyzers — unit tests against hand-built graphs.**

Analyzers do no I/O ([04-analyzers.md](04-analyzers.md)), so they can be tested by constructing a
graph in code and asserting the findings. This is where algorithmic correctness is established: a
three-node cycle, two overlapping SCCs, a self-loop, a diamond with compatible versions, a diamond
with incompatible ones. No files, no fixtures, fast, and the expected answer is knowable by
construction. **The majority of tests should live here.**

**Parsers — table-driven tests over real manifest snippets.**

For each format, a set of `(input, expected deps)` pairs taken from real files, including the ugly
ones: workspace inheritance, parent POMs, optional and peer deps, git and path dependencies, dep
groups. Every parser bug found in the wild becomes a new row.

**Import extraction — per-language corpora.**

A file per language exercising every import form the grammar allows: aliased, nested, conditional,
type-only, multi-line, re-export, dynamic, and imports appearing inside strings and comments (which
must *not* be extracted). The last category is the one regex-based extraction fails and is worth
asserting explicitly.

**Import resolution — the mapping table is itself a test.**

Every row of the exception table in [03-language-providers.md](03-language-providers.md) is a test
case. Add the structural rules too: `serde-json`↔`serde_json`, `@scope/pkg/deep` → `@scope/pkg`,
Go's longest-prefix rule.

**End to end — fixture repositories.**

Small, checked-in, deliberately broken projects — one per language, each containing a known cycle, a
known unused dep, a known missing dep, with a lockfile committed. Assert the whole JSON report via
snapshot (`insta`). These are also the regression suite: a bug report becomes a fixture.

Keep fixtures *small*. A fixture that takes a minute to analyze will be run rarely and will rot.

**The unavailable path needs fixtures too.** A project with a manifest and no lockfile must produce
`Unavailable` for the resolved-tree checks and real findings for the rest. This is a normal, common
situation and it deserves the same test coverage as the happy path — it is also the easiest behaviour
to break accidentally.

## Determinism

Principle 5 in [README.md](README.md) is testable directly: analyze a fixture twice and assert
byte-identical output. Then analyze with the file-walk order shuffled and assert the same. Given that
parsing is parallel, ordering bugs are near-certain at some point, and they surface as flaky snapshot
tests that are painful to diagnose after the fact. Add this test before adding parallelism.

## Correctness on real repositories

Fixtures prove gdep does what we intended. They cannot prove the intent matches reality. So,
separately from CI:

- Run against a set of well-known open-source repositories per language and **review the findings by
  hand**. The metric that matters is the false-positive rate on unused/missing dependencies, since
  that is what determines whether anyone leaves the tool switched on.
- **Track the import resolution rate** ([03-language-providers.md](03-language-providers.md)) as a
  headline quality number per language. It is the best available proxy for provider completeness and
  it is measurable without an oracle.
- Record wall-clock on the largest repository in the set, so performance regressions are visible.

Treat a language as shippable only once its resolution rate is high and its false-positive rate on a
real repository has been eyeballed. Shipping ten mediocre languages is worse than shipping three
trustworthy ones — a tool that cries wolf on Python gets uninstalled before anyone tries the Go
support.

## What not to test

Do not snapshot-test human-readable text output beyond one smoke test; it will churn constantly for
no correctness benefit. The JSON contract is the thing worth pinning.
