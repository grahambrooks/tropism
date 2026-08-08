# 11 — Dependency rules

tropism accepts a ruleset that constrains what may depend on what, and enforces it alongside the
general checks. Two kinds of rule:

- **Module rules** — the architecture the team intends. "The CLI and the MCP server must not depend
  on each other, but both may depend on the shared core."
- **Package rules** — dependency policy. Approved packages, discouraged ones, and which parts of the
  repository may use them.

Prior art: NDepend, JDepend, ArchUnit, dependency-cruiser, import-linter, `cargo-deny`. Every one is
single-language. tropism's opening is a uniform ruleset across a polyglot repository, checked
hermetically, and queryable by an agent.

## Why this is the strongest thing tropism can do

[10-js-evaluation.md](10-js-evaluation.md) measured a 63% false-positive rate for manifest hygiene
and near-perfect accuracy for cycle detection. The reason for the split is worth stating precisely,
because it predicts that rules will behave like cycles rather than like hygiene.

**Hygiene asks tropism to prove a negative.** "`lodash` is never used" requires having seen *every*
possible use, and uses hide in HTML `<script>` tags, config files, framework strings, and spawn
arguments. Absence is unprovable without an installed tree.

**Rules ask tropism to prove a positive.** "The CLI imports the MCP server" is a fact about a line of
source that either exists or does not. Same for "this file imports a banned package". There is no
hidden channel through which a violation could occur invisibly — a violation *is* an import, and
imports are unambiguous syntax.

This puts rule violations in the same soundness class as cycles:

|                   | Input                               | Detects        | Sound offline? |
| ----------------- | ----------------------------------- | -------------- | -------------- |
| `unused-dep`      | absence of imports                  | a negative     | no — 63% FP    |
| `cycle`           | presence of imports                 | a positive     | yes            |
| **module rules**  | presence of imports                 | **a positive** | **yes**        |
| **package rules** | presence of a declaration or import | **a positive** | **yes**        |

Three further properties matter:

- **No heuristics.** The ruleset is written by the team. tropism is not inferring intent, so there is
  nothing to be wrong about. A violation cites the rule and the line; both are checkable.
- **The general checks cannot express it.** No amount of cycle detection tells you that `tropism-lang`
  must not reach into `tropism`. That constraint exists only in the team's head until it is
  written down.
- **It fails closed.** A rule that cannot be evaluated reports `Unavailable`, exactly like every
  other check. Silence never means compliance.

One caveat, stated up front: a `require` rule ("A must depend on B") *is* a negative assertion and
inherits hygiene's weakness. It is specified below and marked accordingly.

## Rules span projects, which forces the repo-wide graph

The motivating example is this repository: `tropism` and `tropism-mcp` must not depend on each other,
and both may depend on `tropism-core`. Each of those is a separate crate with its own `Cargo.toml`, so
each is a separate **project** in tropism's model — and
[01-architecture.md](01-architecture.md) analyzes projects independently.

**A rule engine cannot work per-project.** It needs one graph spanning the whole scan.

This resolves open question 1 in [07-open-questions.md](07-open-questions.md) by forcing it: tropism
must build a repository-wide module graph in which a project is itself a node. The per-project
graphs remain for cycle detection within a project; the repo-wide graph is what rules are evaluated
against.

Dependencies between projects arrive at two levels, and **both are checked**:

- **Declared** — `tropism`'s `Cargo.toml` lists `tropism-core`. An edge exists even if no source file
  has imported it yet.
- **Imported** — a file in `crates/tropism/` has `use tropism_core::…`.

A rule violated only at the declaration level is still a violation: the coupling is real and the
import is a commit away. Findings say which level triggered.

## The ruleset file

`tropism.toml` at the scan root, TOML to match the config format already chosen in
[05-interfaces.md](05-interfaces.md). Discovered automatically; `--rules <path>` overrides.

### Exclusions

Paths kept out of the analysis entirely, before anything is discovered or walked.

```toml
exclude = [
  "demo/**",
  "**/tests/fixtures/**",
]
```

The motivating case is a repository containing deliberately-broken sample projects — this one does,
and without exclusions `tropism analyze .` could never return zero, so the repository could not gate CI
on its own rules.

**Exclusions are disclosed in every report**, with a count per pattern, and a pattern matching
nothing is flagged. This is the same discipline as `CheckStatus`: an exclusion is a deliberate blind
spot, and a repository that excluded half of itself must not look like one that was fully analyzed.

```
58 path(s) excluded by tropism.toml
  **/tests/fixtures/** — 21 path(s)
  demo/** — 37 path(s)
```

Note that `tropism.toml` is therefore read twice: once before discovery for the exclusions, and once at
the end for the rules. Exclusions must be known before any file is walked; rules can only be
evaluated after every edge is collected.

### Modules

A module is a name bound to one or more path globs. Modules are tropism's unit of architecture and need
not correspond to projects — they may be coarser (a whole crate) or finer (one directory).

```toml
schema_version = 1

[modules]
core = "crates/tropism-core/**"
lang = "crates/tropism-lang/**"
cli  = "crates/tropism/**"
mcp  = "crates/tropism-mcp/**"

# Several globs, and a package name for cross-project edges.
[modules.render]
paths = ["crates/tropism/src/render/**"]

[modules.docs]
paths = ["design/**", "*.md"]
```

Matching is longest-glob-wins, so `render` above claims files that `cli` would otherwise take.
Ambiguity is resolved deterministically and reported: two globs of equal specificity matching one
file is a ruleset error, not a coin toss.

### Module rules

```toml
# The motivating case. Symmetric: neither may depend on the other.
[[module_rules]]
id = "surfaces-are-independent"
independent = ["cli", "mcp"]
severity = "error"
reason = """
The CLI and MCP server are independent adapters over one analysis core
(design/README.md, principle 4). Shared behaviour belongs in core, not in a
dependency between the two surfaces.
"""

# Directional.
[[module_rules]]
id = "providers-do-not-render"
deny = { from = "lang", to = ["cli", "mcp"] }
severity = "error"
reason = "A language provider must not know how findings are displayed."

# Closed world: core may depend on nothing else in the repository.
[[module_rules]]
id = "core-is-a-leaf"
allow_only = { from = "core", to = [] }
severity = "error"
reason = "tropism-core must stay free of rendering, CLI, and transport concerns."

# Ordered stack; each layer may depend only on those below it.
[[module_rules]]
id = "layering"
layers = ["cli", "lang", "core"]
severity = "error"

# The one negative-shaped rule. See the caveat above.
[[module_rules]]
id = "cli-uses-core"
require = { from = "cli", to = "core" }
severity = "warning"
```

Rule kinds:

| Kind          | Meaning                                        | Shape        |
| ------------- | ---------------------------------------------- | ------------ |
| `deny`        | `from` must not depend on any of `to`          | positive     |
| `independent` | no member may depend on any other member       | positive     |
| `allow_only`  | `from` may depend only on `to` (and itself)    | positive     |
| `layers`      | ordered; each may depend only on later entries | positive     |
| `require`     | `from` must depend on `to`                     | **negative** |

`deny`, `independent`, `allow_only`, and `layers` all reduce to the same question — does an edge
exist that should not — so one analyzer evaluates all four. `require` is the inverse and is reported
at `Confidence::Medium` at best, because an unseen dependency mechanism could satisfy it invisibly.

**Transitive by default is wrong.** A `deny` rule matches a *direct* edge unless `transitive = true`
is set. Direct edges are unambiguous; transitive ones depend on the completeness of the whole graph,
so they are opt-in and reported with the offending path as evidence.

### Package rules

```toml
[packages]
# "allow" (default): anything not mentioned is fine — a denylist.
# "deny": anything not explicitly allowed is a violation — an approved list.
unlisted = "allow"

[[package_rules]]
id = "no-archived-yaml"
deny = ["serde_yaml"]
replacement = "saphyr"
severity = "error"
reason = "serde_yaml is archived; see design/08-crates.md."

[[package_rules]]
id = "tui-stays-in-the-cli"
packages = ["ratatui"]
allowed_in = ["cli"]
severity = "error"
reason = "The interactive browser is a CLI concern; core must stay renderer-agnostic."

[[package_rules]]
id = "prefer-one-http-client"
deny = ["request", "node-fetch"]
replacement = "undici"
severity = "warning"
```

Three capabilities, each a presence check:

- **Denylist** — a named package must not appear. Detected from the manifest declaration *and* from
  any import, so removing it from `package.json` while leaving the import behind still fails.
- **Approved list** — set `unlisted = "deny"` and the ruleset becomes closed-world: every dependency
  must be explicitly allowed. This is the shape regulated teams actually want.
- **Scoped allowance** — `allowed_in` restricts a package to named modules. This is the rule that
  keeps a UI library out of a domain layer, and it is the most useful of the three in practice.

**Version constraints are deferred.** `deny = ["lodash < 4.17.21"]` needs ecosystem-correct version
comparison, and [02-data-model.md](02-data-model.md) keeps versions opaque behind `VersionOps`,
which no provider implements yet. Specify the syntax now, reject it with a clear error until
`VersionOps` is real, and do not ship a lexical comparison.

**Transitive bans need a resolved tree.** "Nothing may pull in `left-pad`, even indirectly" is
answerable for npm and *not* for Go, where `go.sum` is not a resolved graph. That check reports
`Unavailable` with the ecosystem reason, exactly as `version-conflict` already does.

**License policy is out of scope.** `cargo-deny`'s licence checking needs each dependency's licence
metadata, which lives in the registry or in an installed tree — neither of which tropism may read.
Saying so is better than a half-implementation that silently misses most dependencies.

## Implementation status

Implemented and enforced: `exclude`, `[[workspaces]]`, `deny`, `independent`, `allow_only`,
`crosses_workspace`, package denylists with `replacement`, `allowed_in` scoping, closed-world
approved lists (`unlisted = "deny"`), and stale-rule detection.

`crosses_workspace = true` is the one rule kind that **names no module**. It forbids any edge leaving
its workspace, where the boundary comes from `crate::workspace` — the ecosystem's own declaration, or
`[[workspaces]]`, or the language fallback — rather than from a glob somebody has to keep in step
with the repository layout. Three consequences:

- Nothing to update when a directory is renamed, which is the usual way a module-named rule rots.
- `crosses_workspace = false` is a **parse error**. It would enforce nothing while looking like it
  enforced something, which is the failure this whole section exists to prevent.
- It is **stale in a single-workspace repository**, since it cannot fire there. Same treatment as a
  rule naming a module that matches nothing, and the likelier mistake — the rule is easy to write
  before any boundary exists to enforce.

It exists because a cross-workspace import is a real defect that tropism should not unilaterally
assign a severity to: it resolves today through hoisting and breaks when the package is published or
built alone, and whether that blocks a commit is the team's judgement. That is precisely the argument
for a rule over an inferred check.

Not implemented: `layers`, `require`, `transitive`, and version constraints. These are **rejected at
parse time with an error naming the field**, rather than ignored — a ruleset must never appear to
enforce more than it does.

This repository's own ruleset is [`tropism.toml`](../tropism.toml), and
`crates/tropism-lang/tests/demos.rs` asserts tropism satisfies it. Each demo under `demo/` carries a
ruleset with both a satisfied and a violated rule.

Two behaviours settled by building it:

- **Staleness keys on whether a rule's modules match any path**, not on whether the rule fired. The
  first attempt marked a rule live whenever *any* cross-module edge existed anywhere, which would
  never have caught the renamed-module case the check exists for.
- **`allowed_in` applies to imports, not declarations.** A manifest entry has no module location, so
  scoping it produced "used in an unassigned path" — a statement about nothing.

## Semantics that must be pinned down

**Unmatched files.** A file matching no module belongs to the implicit module `unassigned`. Under
`unlisted = "allow"` it is ignored. A `strict_modules = true` setting makes an unassigned file an
error, which is what a team ratcheting toward full coverage wants.

**Rule precedence.** Rules do not override one another; every rule is evaluated and each violation is
its own finding. A dependency forbidden by two rules produces two findings, because suppressing one
should not silently suppress the other.

**Self-dependency.** A module never violates a rule by depending on itself.

**Stale rules are reported.** A rule that matched no module, or a `deny` naming a package absent from
the repository, is emitted as an `Info` finding. Rulesets rot: a module gets renamed and the rule
protecting it silently stops doing anything. This is the same reasoning as `CheckStatus` — silence
must never be mistaken for compliance.

**Confidence.** Rule violations are `Confidence::High`. The input is declarative, the evidence is a
line of source, and no inference is involved. This is the first check other than `cycle` that earns
High.

**Severity defaults to `error`.** The team asserted the rule; violating it is not a suggestion.

## Findings

Two new checks, joining the six in [04-analyzers.md](04-analyzers.md):

| Check          | Requires                                              | Confidence ceiling |
| -------------- | ----------------------------------------------------- | ------------------ |
| `module-rule`  | repo-wide module graph                                | High               |
| `package-rule` | manifests + imports (+ resolved tree if `transitive`) | High               |

A violation cites both sides — the rule that was broken and the code that broke it:

```
error[module-rule:.:a1b2c3]: `cli` must not depend on `mcp` (rule: surfaces-are-independent)
  --> crates/tropism/src/render/tui.rs:14:1
   |
14 | use tropism_mcp::protocol::Finding;
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ imports `mcp`
   |
  ::: tropism.toml:12:1
   |
12 | independent = ["cli", "mcp"]
   | --------------------------- rule defined here
   |
   = note: The CLI and MCP server are independent adapters over one analysis core.
   = confidence: high
```

The `reason` field is rendered verbatim. It is the most valuable part of the output: it tells a
developer — or an agent — *why* the constraint exists, which no inferred finding can ever do. Rules
without a `reason` should draw a lint of their own.

`details` carries `rule_id`, `from`, `to`, and `level` (`declared` or `imported`), so an agent can
act without parsing prose.

## Interfaces

**CLI.** `tropism check` runs rules only — the fast, high-signal subset suitable for a pre-commit hook.
`tropism analyze` runs everything. `--rules <path>` overrides discovery; `--no-rules` disables.

Two inspection surfaces, neither of which is a check and both of which exit 0 unless the run itself
failed. They exist because a wrong *input* to a rule produces a silent wrong answer, and until they
existed the only way to see that input was to read tropism's source:

- **`tropism workspaces`** — the boundaries, the file each was read from, and every dependency
  crossing one. The origin field is the part to read: `language` means tropism inferred the grouping
  because the ecosystem declares nothing, and that is the case most likely to be wrong.
- **`tropism explain <file>`** — every import in one file, what it resolved to, and one sentence
  saying why: declared, exempted by a named workspace sibling, hoisted from an ancestor, stdlib,
  genuinely missing, or unresolved. `--import <spec>` narrows it. An `unresolved` import contributes
  no edge, so it is the usual answer to "why did my rule not fire".

**Exit codes** are unchanged. Because rule violations default to `error`, `--fail-on error` gates
them by default while the noisier general checks stay advisory — which is the correct division
given [10-js-evaluation.md](10-js-evaluation.md).

**MCP.** One new tool, `tropism_rules`, returning the active ruleset with each rule's status
(`satisfied`, `violated`, `stale`). An agent about to add a dependency should be able to ask whether
it is permitted *before* writing the import, which is a materially better interaction than being
told afterwards. This is the first genuinely agent-shaped capability in the product.

## Open questions

1. **Do module rules apply to external packages too?** `deny = { from = "core", to = "cli" }` is
   internal. Restricting which modules may use `serde_json` is `allowed_in` on a package rule. The
   two syntaxes overlap and might be better unified.
2. **Inheritance in a monorepo.** Does `packages/web/tropism.toml` extend the root ruleset or replace
   it? Extending is more useful and harder to reason about. Recommend extend, with an explicit
   `inherit = false` escape.

   *Unchanged by `[[workspaces]]`.* That block draws workspace **boundaries** — which projects may
   import each other's published names undeclared — and does not split the ruleset. One
   `tropism.toml` at the scan root still governs everything.
3. **Ratcheting.** Teams adopt these tools on codebases that already violate the rules. A
   `baseline` file recording accepted violations — failing only on *new* ones — is what makes
   adoption possible at all, and it should probably ship with the first version rather than after.
4. **Glob dialect.** `globset` is already a transitive dependency via `ignore`. Confirm its
   semantics match what users expect from `**` before committing to it in a file format.

5. ~~**Should the ruleset be a general policy language?**~~ **Evaluated and declined for now** —
   [18-rego-evaluation.md](18-rego-evaluation.md). Rego via `regorus` was prototyped and works,
   hermetically, at acceptable cost. It was declined because the only gap it closes is D6/D7, which
   is less work to implement natively than to buy a second rule vocabulary — and because an
   arbitrary policy cannot be scoped to a change the way a per-edge rule can, which would make
   `tropism check` a liar about policies it could not evaluate.

## Revised product thesis

[09-product-review.md](09-product-review.md) concluded that "detects dependency problems" is a weak
proposition because every ecosystem's native tooling already does it, better, with autofix.
[10-js-evaluation.md](10-js-evaluation.md) then measured hygiene at 63% false positives while cycle
detection held up.

Rules change the proposition. **No native tool can enforce an architecture it was never told about.**
`go mod tidy` cannot know that the CLI must not import the MCP server. That constraint lives in a
file the team writes, and enforcing it is:

- sound under the hermetic constraint, because it is a presence check;
- unavailable from any incumbent, because the incumbents are single-language and none is
  agent-facing;
- built on the module graph that has already been proven on two ecosystems.

Suggested revision to the build order in [07-open-questions.md](07-open-questions.md): rules move
ahead of both the remaining languages and the MCP server, because they are the first feature whose
value does not depend on beating an existing tool at its own game.
