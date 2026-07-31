# 11 — Dependency rules

gdep accepts a ruleset that constrains what may depend on what, and enforces it alongside the
general checks. Two kinds of rule:

- **Module rules** — the architecture the team intends. "The CLI and the MCP server must not depend
  on each other, but both may depend on the shared core."
- **Package rules** — dependency policy. Approved packages, discouraged ones, and which parts of the
  repository may use them.

Prior art: NDepend, JDepend, ArchUnit, dependency-cruiser, import-linter, `cargo-deny`. Every one is
single-language. gdep's opening is a uniform ruleset across a polyglot repository, checked
hermetically, and queryable by an agent.

## Why this is the strongest thing gdep can do

[10-js-evaluation.md](10-js-evaluation.md) measured a 63% false-positive rate for manifest hygiene
and near-perfect accuracy for cycle detection. The reason for the split is worth stating precisely,
because it predicts that rules will behave like cycles rather than like hygiene.

**Hygiene asks gdep to prove a negative.** "`lodash` is never used" requires having seen *every*
possible use, and uses hide in HTML `<script>` tags, config files, framework strings, and spawn
arguments. Absence is unprovable without an installed tree.

**Rules ask gdep to prove a positive.** "The CLI imports the MCP server" is a fact about a line of
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

- **No heuristics.** The ruleset is written by the team. gdep is not inferring intent, so there is
  nothing to be wrong about. A violation cites the rule and the line; both are checkable.
- **The general checks cannot express it.** No amount of cycle detection tells you that `gdep-lang`
  must not reach into `gdep-cli`. That constraint exists only in the team's head until it is
  written down.
- **It fails closed.** A rule that cannot be evaluated reports `Unavailable`, exactly like every
  other check. Silence never means compliance.

One caveat, stated up front: a `require` rule ("A must depend on B") *is* a negative assertion and
inherits hygiene's weakness. It is specified below and marked accordingly.

## Rules span projects, which forces the repo-wide graph

The motivating example is this repository: `gdep-cli` and `gdep-mcp` must not depend on each other,
and both may depend on `gdep-core`. Each of those is a separate crate with its own `Cargo.toml`, so
each is a separate **project** in gdep's model — and
[01-architecture.md](01-architecture.md) analyzes projects independently.

**A rule engine cannot work per-project.** It needs one graph spanning the whole scan.

This resolves open question 1 in [07-open-questions.md](07-open-questions.md) by forcing it: gdep
must build a repository-wide module graph in which a project is itself a node. The per-project
graphs remain for cycle detection within a project; the repo-wide graph is what rules are evaluated
against.

Dependencies between projects arrive at two levels, and **both are checked**:

- **Declared** — `gdep-cli`'s `Cargo.toml` lists `gdep-core`. An edge exists even if no source file
  has imported it yet.
- **Imported** — a file in `crates/gdep-cli/` has `use gdep_core::…`.

A rule violated only at the declaration level is still a violation: the coupling is real and the
import is a commit away. Findings say which level triggered.

## The ruleset file

`gdep.toml` at the scan root, TOML to match the config format already chosen in
[05-interfaces.md](05-interfaces.md). Discovered automatically; `--rules <path>` overrides.

### Modules

A module is a name bound to one or more path globs. Modules are gdep's unit of architecture and need
not correspond to projects — they may be coarser (a whole crate) or finer (one directory).

```toml
schema_version = 1

[modules]
core = "crates/gdep-core/**"
lang = "crates/gdep-lang/**"
cli  = "crates/gdep-cli/**"
mcp  = "crates/gdep-mcp/**"

# Several globs, and a package name for cross-project edges.
[modules.render]
paths = ["crates/gdep-cli/src/render/**"]

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
reason = "gdep-core must stay free of rendering, CLI, and transport concerns."

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
metadata, which lives in the registry or in an installed tree — neither of which gdep may read.
Saying so is better than a half-implementation that silently misses most dependencies.

A complete worked example for this repository ships as
[`gdep.toml.example`](../gdep.toml.example) — the rules above are the ones gdep should be enforcing
on itself.

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
  --> crates/gdep-cli/src/render/tui.rs:14:1
   |
14 | use gdep_mcp::protocol::Finding;
   | ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ imports `mcp`
   |
  ::: gdep.toml:12:1
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

**CLI.** `gdep check` runs rules only — the fast, high-signal subset suitable for a pre-commit hook.
`gdep analyze` runs everything. `--rules <path>` overrides discovery; `--no-rules` disables.

**Exit codes** are unchanged. Because rule violations default to `error`, `--fail-on error` gates
them by default while the noisier general checks stay advisory — which is the correct division
given [10-js-evaluation.md](10-js-evaluation.md).

**MCP.** One new tool, `gdep_rules`, returning the active ruleset with each rule's status
(`satisfied`, `violated`, `stale`). An agent about to add a dependency should be able to ask whether
it is permitted *before* writing the import, which is a materially better interaction than being
told afterwards. This is the first genuinely agent-shaped capability in the product.

## Open questions

1. **Do module rules apply to external packages too?** `deny = { from = "core", to = "cli" }` is
   internal. Restricting which modules may use `serde_json` is `allowed_in` on a package rule. The
   two syntaxes overlap and might be better unified.
2. **Inheritance in a monorepo.** Does `packages/web/gdep.toml` extend the root ruleset or replace
   it? Extending is more useful and harder to reason about. Recommend extend, with an explicit
   `inherit = false` escape.
3. **Ratcheting.** Teams adopt these tools on codebases that already violate the rules. A
   `baseline` file recording accepted violations — failing only on *new* ones — is what makes
   adoption possible at all, and it should probably ship with the first version rather than after.
4. **Glob dialect.** `globset` is already a transitive dependency via `ignore`. Confirm its
   semantics match what users expect from `**` before committing to it in a file format.

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
