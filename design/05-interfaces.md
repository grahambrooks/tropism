# 05 — Interfaces

Both surfaces are adapters. Neither contains analysis logic, and any question answerable through one
is answerable through the other with an identical result.

## JSON output contract

The JSON serialization of `Report` ([02-data-model.md](02-data-model.md)) is the contract both
surfaces are built on, and it is a public API — version it from day one.

It is produced by `Report::to_json_pretty` in `tropism-core`, and **both the CLI and the MCP server
call it** rather than each serializing independently. One implementation means an agent reading an
MCP response and a human reading piped CLI output cannot be looking at different shapes.

```json
{
  "schema_version": 1,
  "tropism_version": "0.1.0",
  "scan_root": ".",
  "projects": [
    {
      "root": "backend",
      "language": "python",
      "checks": {
        "cycle":            { "status": "ran", "finding_count": 2 },
        "unused-dep":       { "status": "ran", "finding_count": 1 },
        "version-conflict": { "status": "unavailable",
                              "reason": "no lockfile found; resolved tree unknown" }
      },
      "findings": [
        {
          "id": "cycle:backend:a1b2c3",
          "check": "cycle",
          "severity": "warning",
          "confidence": "high",
          "message": "Import cycle among 3 modules",
          "evidence": [
            { "file": "backend/api/user.py",  "line": 3, "note": "imports backend.api.order" },
            { "file": "backend/api/order.py", "line": 7, "note": "imports backend.api.billing" }
          ],
          "details": { "members": ["backend/api/user.py", "backend/api/order.py"] }
        }
      ]
    }
  ],
  "skipped": [
    { "file": "vendor/legacy.py", "reason": "parse error at line 42" }
  ]
}
```

Rules: paths relative to `scan_root`; no absolute paths, no timestamps, no machine-specific content
anywhere in the payload. `schema_version` increments on any breaking change. Additive fields are not
breaking; consumers must ignore unknown fields.

## CLI

```
tropism analyze [PATH]              # all checks, default command
tropism check   [PATH]              # ruleset only: fast, high-signal, hook-friendly
tropism cycles  [PATH]              # single check, for the common fast case
tropism graph   [PATH]              # export the graph, do not analyze

Options:
  --format auto|text|json|tui|sarif   # auto = text on a tty, json when piped
  --check <id>...                # restrict to named checks
  --language <lang>...           # restrict to languages
  --severity <min>               # filter output
  --fail-on <severity>           # exit-code threshold for CI
  --config <path>
  --rules <path>                 # override tropism.toml discovery
  --no-rules                     # skip the ruleset entirely
  --no-ignore                    # do not honour .gitignore
```

`tropism check` exists because rule violations default to `error` while the general checks are
advisory. It is the subset a pre-commit hook or a merge gate should run, and it accepts a list of
files so it can check only what changed — specified in
[14-incremental-checking.md](14-incremental-checking.md), which argues it is the product's strongest
differentiator.

**Exit codes** are the CI contract and must be distinguishable:

| Code | Meaning                                           |
| ---- | ------------------------------------------------- |
| 0    | Ran; nothing at or above `--fail-on`              |
| 1    | Ran; findings at or above `--fail-on`             |
| 2    | Could not run (bad path, bad config, no projects) |

Conflating 1 and 2 means a broken invocation looks like a passing build. Keep them separate.

`--format sarif` is what gets findings into GitHub code scanning and most CI annotation systems. It
is a mechanical mapping from the JSON above and is worth having early.

Text output leads with per-project check status — including anything unavailable — before findings,
so a human sees "3 checks did not run" rather than reading a short list as a clean bill of health.

### The interactive browser (`--format tui`)

A `ratatui` two-pane browser: a navigable list of projects, findings, and unavailable checks on the
left; full detail with evidence on the right.

It is **opt-in, never the default**, and `--format auto` never resolves to it. An alternate-screen
UI cannot be piped, redirected, or read by CI, and those three have to keep working. Redirecting
`--format tui` is an error (exit 2) rather than a screenful of escape codes.

Two properties make it maintainable rather than a second untested surface:

- **The state machine is separate from drawing.** Navigation is a pure function over a row list, so
  it is unit-tested without a terminal.
- **Layout is snapshot-tested** against `ratatui`'s `TestBackend`, using the same fixture report as
  the text renderer, so the two renderers cannot silently disagree about what a report contains.

Unavailable checks are listed as rows alongside findings, not hidden in a status line. Selecting one
says in as many words that the check produced no answer — an interactive view that quietly omitted
them would be the easiest possible way to read an unanalyzed repo as a clean one.

It is feature-gated (`--no-default-features` drops `ratatui` entirely), so the non-interactive
formats stay buildable without the heaviest dependency in the workspace.

## MCP server

**Status: last in the build order, and deliberately smaller than first specified.**
[09-product-review.md](09-product-review.md) once made this surface the product. Its own gate —
"validate that `tropism_package_path` is answerable" — failed once all ten languages existed: four
ecosystems have no lockfile edges at all, so the flagship query is dead in four of ten supported
languages. The revision is recorded there; the tool list below is what survives it.

The consumer is an agent with a limited context window. That single fact drives the design:
**an MCP tool that returns the full report on a large monorepo is useless**, because it will blow the
context it was supposed to inform. Tools are therefore narrow, filtered, and paginated by default.

### The three tools worth shipping

| Tool              | Purpose                                                                 |
| ----------------- | ----------------------------------------------------------------------- |
| `tropism_summary` | Counts per check per project, plus which checks could not run. Cheap.   |
| `tropism_check`   | The rules, evaluated against a file list. The same call the hook makes. |
| `tropism_rules`   | The active ruleset, each rule satisfied/violated/stale, with `reason`.  |

`tropism_check` is the one to build first, and it is not in the original list because the subcommand
it wraps did not exist yet. It is the same question the pre-commit hook asks — "does this change
break a rule?" — and asking it *before* writing an import is strictly more useful to an agent than
being told afterwards. It also costs almost nothing once
[14-incremental-checking.md](14-incremental-checking.md) is built, because it is a thin adapter over
`tropism check` rather than a second implementation.

`tropism_rules` is worth keeping but is worth less than it looks: `tropism.toml` is a small TOML file
with the reasons written in it, and an agent can simply read it. What the tool adds over the file is
evaluation — *satisfied, violated, or stale* — which the file cannot state about itself.

### Deferred, with reasons

| Tool                   | Why it waits                                                                                                                    |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `tropism_package_path` | Needs a resolved tree. Unavailable in Go, Java, Swift, and C++; misleading even in Cargo, where a path may run through a feature no build compiles (S8). |
| `tropism_findings`     | Two of six checks measured 63% false positives. Handing those to an agent is worse than handing them to a human, who reads before acting. |
| `tropism_explain`      | Only worth building once there is a finding stream worth explaining.                                                            |
| `tropism_module_deps`  | Genuinely useful and genuinely unbuilt; no evidence yet that an agent wants it.                                                 |

The full surface as originally proposed, kept because the reasoning behind each entry is still
sound and the deferral is about sequencing rather than merit:

| Tool                   | Purpose                                                        |
| ---------------------- | -------------------------------------------------------------- |
| `tropism_summary`      | Counts per check per project. Cheap. The intended entry point. |
| `tropism_findings`     | Findings, filtered by check/severity/path, paginated.          |
| `tropism_explain`      | Full evidence and detail for one finding ID.                   |
| `tropism_module_deps`  | What one module imports, and what imports it.                  |
| `tropism_package_path` | Why a package is in the tree: the paths from root to it.       |
| `tropism_check_status` | Which checks ran, which could not, and why.                    |
| `tropism_rules`        | The active ruleset, each rule satisfied/violated/stale.        |

Design rules for these tools:

- **`tropism_summary` first.** An agent should orient with counts, then drill in. Make the summary
  response small enough to be free.
- **Every list tool paginates**, with a documented default cap and an explicit indication when
  results were truncated. Silent truncation would let an agent conclude a codebase is clean when it
  saw the first page.
- **Responses carry the same `Finding` shape as the JSON contract**, so an agent that has seen CLI
  output recognizes MCP output.
- **State the unavailable checks in every response** that could be misread without them. An agent
  cannot infer that a check never ran; if `tropism_findings` returns nothing, it must be told whether
  that means clean or unavailable.
- **Analysis is cached per scan root** within a server session, so `summary` followed by three
  `findings` calls does not re-analyze the tree four times. Invalidate on file mtime change.

`tropism_package_path` was written up here as the flagship: "why is this package here?" is the
question an agent most often needs answered when acting on a dependency finding, and it is exactly
the question a resolved tree answers precisely. **That reasoning was sound and the premise was
wrong** — six of the ten ecosystems cannot supply the tree, so the query cannot be the flagship of a
ten-language tool. It is deferred rather than dropped: for a repository that is purely npm or purely
Cargo it remains the best question on this list.

`tropism_rules` was described here as "the first capability genuinely better as an agent interface
than as a CLI", on the grounds that it lets an agent ask **before** acting. The instinct was right
and the surface was wrong: what an agent wants before acting is not the rule list but the *verdict*
on a specific change, which is `tropism_check`. Asking "may this module depend on that one?" is one
`tropism check` call over the file about to be written.

## Configuration

A single optional file at the scan root — format follows the ecosystem convention (TOML). It covers
severity overrides, suppressions ([04-analyzers.md](04-analyzers.md)), extra ignore paths, and which
checks are enabled. CLI flags override the file; the file overrides defaults. The MCP server reads
the same file, so both surfaces agree.

The ruleset lives in `tropism.toml` at the scan root and is specified separately in
[11-dependency-rules.md](11-dependency-rules.md). Keeping it distinct from tool configuration is
deliberate: the ruleset is a description of the architecture, reviewed like source, while the config
is knobs. They have different audiences and different change rates.
