# tropism design specification

**Every limitation found while building this is registered in
[12-known-limitations.md](12-known-limitations.md)**, split into those that are structural — the
price of never invoking a package manager, reported rather than fixed — and those merely deferred.
Read it before planning work, and add to it rather than discovering the same gap twice.

**Status:** four vertical slices are complete — Go, JavaScript/TypeScript, and Rust — plus the
dependency ruleset. tropism is run against itself, and against a deliberately-broken demo project per
language. The remaining seven languages and the MCP server are not built.

Previously: two vertical slices are complete — Go, and JavaScript/TypeScript with a real resolved
tree, so all six checks have now run. The remaining eight languages and the MCP server are not
built, and everything about them here is still intent rather than a description of behaviour.

**Read [09-product-review.md](09-product-review.md) and [10-js-evaluation.md](10-js-evaluation.md)
before planning further work.** Both slices contradicted parts of the plan below. In particular,
manifest hygiene measured a 63% false-positive rate on real JavaScript repositories and should not
ship on by default, while cycle detection proved sound and is the strongest remaining claim.

[11-dependency-rules.md](11-dependency-rules.md) specifies the team-authored ruleset — now
implemented, and enforced on this repository via [`tropism.toml`](../tropism.toml). It detects the presence
of a forbidden edge rather than the absence of a use, which places it in the same soundness class as
cycle detection, and no native tool can enforce an architecture it was never told about.

These documents define what tropism should be before it is built. They exist so that implementation
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
| [10-js-evaluation.md](10-js-evaluation.md)           | The kill-criterion run on ten real JS/TS repositories    |
| [11-dependency-rules.md](11-dependency-rules.md)     | Team-defined architecture and package policy rules       |

## Design principles

These are the tiebreakers. When a decision in a later document seems arbitrary, it usually traces
back to one of these.

**1. Never execute the analyzed repository.**

The rule in [CLAUDE.md](../CLAUDE.md) against invoking native package managers is not only about
avoiding a toolchain dependency — it also means tropism never runs code it is analyzing. `build.gradle`,
`Package.swift`, `conanfile.py`, and `setup.py` are programs. tropism reads them as text and accepts
that it will sometimes read them incompletely. This is a safety property, and it is not traded away
for coverage.

**2. Partial results beat no results.**

A repository with a `Cargo.toml` and no `Cargo.lock` still gets cycle detection and manifest
hygiene. A monorepo with eight parseable projects and two unparseable ones reports on eight and says
so. Analysis is per-check and per-project; one failure never aborts the run.

**3. Confidence is part of the output, not a footnote.**

Every finding carries a confidence level and the evidence behind it. A check that cannot run says
*unavailable* and why. tropism never presents a guess as a fact — an agent consuming this over MCP
cannot tell the difference unless we mark it.

**4. One core, two surfaces.**

The CLI and MCP server are thin adapters over the same analysis library. No analysis logic lives in
either. If a question can be answered by the CLI, the same question is answerable over MCP with the
same result.

**5. Deterministic, diffable output.**

Same input, same version, same bytes out. Findings are sorted by a stable key; no map iteration
order, no timestamps in the payload, no absolute paths in the default output. This is what makes
tropism usable in CI and in snapshot tests.

**6. Speed is a feature.**

The target is a large repository analyzed in seconds, because a check that is slow gets run rarely,
and a check that is run rarely stops being trusted. Parse work is per-file and parallel.

## Scope boundary

tropism reports problems. It does not fix them: no manifest rewriting, no import rewriting, no
`--fix`. Findings should carry enough structure for an agent to act on them, and acting is the
agent's job. Revisit only after the detection side is proven.
