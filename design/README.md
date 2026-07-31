# gdep design specification

**Status:** the Go vertical slice is complete — discovery, `go.mod` parsing, import extraction, the
module graph, and three of six analyzers, verified against five real repositories. The other nine
languages and the MCP server are not built, and everything about them here is still intent rather
than a description of behaviour.

**Read [09-product-review.md](09-product-review.md) before planning further work.** Building the
slice contradicted parts of the plan below, most importantly the choice of Go as the first language
and the assumption that the six checks are where the value lies.

These documents define what gdep should be before it is built. They exist so that implementation
work can start anywhere without re-deriving the same decisions, and so that a decision that turns
out to be wrong can be found and changed in one place.

Where the build has already contradicted a document, the document has been corrected rather than
annotated — see the `LanguageProvider` trait's location in
[01-architecture.md](01-architecture.md).

## Reading order

| Document                                             | Answers                                                  |
| ---------------------------------------------------- | -------------------------------------------------------- |
| [01-architecture.md](01-architecture.md)             | How the system is layered, and what runs in what order   |
| [02-data-model.md](02-data-model.md)                 | The core types every layer passes around                 |
| [03-language-providers.md](03-language-providers.md) | How a language is added, and the import→package problem  |
| [04-analyzers.md](04-analyzers.md)                   | Each check: algorithm, inputs required, failure modes    |
| [05-interfaces.md](05-interfaces.md)                 | CLI surface, MCP surface, JSON output contract           |
| [06-testing.md](06-testing.md)                       | How correctness is established for a tool with no oracle |
| [07-open-questions.md](07-open-questions.md)         | Decisions deferred, and what they block                  |
| [08-crates.md](08-crates.md)                         | Verified dependency choices, and the gaps with no answer |
| [09-product-review.md](09-product-review.md)         | Is this worth building? Evidence from the Go slice       |

## Design principles

These are the tiebreakers. When a decision in a later document seems arbitrary, it usually traces
back to one of these.

**1. Never execute the analyzed repository.**

The rule in [CLAUDE.md](../CLAUDE.md) against invoking native package managers is not only about
avoiding a toolchain dependency — it also means gdep never runs code it is analyzing. `build.gradle`,
`Package.swift`, `conanfile.py`, and `setup.py` are programs. gdep reads them as text and accepts
that it will sometimes read them incompletely. This is a safety property, and it is not traded away
for coverage.

**2. Partial results beat no results.**

A repository with a `Cargo.toml` and no `Cargo.lock` still gets cycle detection and manifest
hygiene. A monorepo with eight parseable projects and two unparseable ones reports on eight and says
so. Analysis is per-check and per-project; one failure never aborts the run.

**3. Confidence is part of the output, not a footnote.**

Every finding carries a confidence level and the evidence behind it. A check that cannot run says
*unavailable* and why. gdep never presents a guess as a fact — an agent consuming this over MCP
cannot tell the difference unless we mark it.

**4. One core, two surfaces.**

The CLI and MCP server are thin adapters over the same analysis library. No analysis logic lives in
either. If a question can be answered by the CLI, the same question is answerable over MCP with the
same result.

**5. Deterministic, diffable output.**

Same input, same version, same bytes out. Findings are sorted by a stable key; no map iteration
order, no timestamps in the payload, no absolute paths in the default output. This is what makes
gdep usable in CI and in snapshot tests.

**6. Speed is a feature.**

The target is a large repository analyzed in seconds, because a check that is slow gets run rarely,
and a check that is run rarely stops being trusted. Parse work is per-file and parallel.

## Scope boundary

gdep reports problems. It does not fix them: no manifest rewriting, no import rewriting, no
`--fix`. Findings should carry enough structure for an agent to act on them, and acting is the
agent's job. Revisit only after the detection side is proven.
