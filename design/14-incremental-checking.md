# 14 — Incremental checking and the pre-commit hook

Checking only what changed, against the rules, in the moment the developer is about to commit it.

This is the strongest differentiator in the product, and it falls out of properties tropism already
has rather than requiring new ones. It also resolves the adoption problem that
[12-known-limitations.md](12-known-limitations.md) records as D8.

---

## Why this is the differentiator

Three things have to be true for an architecture check to work as a pre-commit hook, and no existing
tool has all three.

**1. It must run in well under a second.** A hook that takes five seconds gets bypassed with
`--no-verify` within a week. tropism analyzes 726 Go files in 1.3 s doing *everything*; checking six
changed files against the rules is a different order of magnitude.

**2. It must need no build, no install, and no network.** This is the property that looked like a
liability in [10-js-evaluation.md](10-js-evaluation.md) — the hermetic constraint is precisely why
`unused-dep` cannot be trusted. For a pre-commit hook it is the whole ballgame. ArchUnit is a JUnit
test: it needs a compiled classpath. NDepend needs a built solution. `dependency-cruiser` needs
`node_modules`. tropism needs a directory.

**The constraint that makes the weakest check unreliable is what makes the strongest one
deployable.** That is worth stating plainly, because it is the argument for the whole design.

**3. It must be sound.** A hook that cries wolf gets disabled, permanently, by the first developer it
blocks wrongly. Rule violations are *presence* checks over import syntax — the same soundness class
as cycle detection, and the reason [11-dependency-rules.md](11-dependency-rules.md) exists.
`unused-dep`, at 63% false positives, must never run in a hook.

Cross-language is the fourth thing, and nothing else offers it at all: one hook, one ruleset, for a
repository with a Go backend, a TypeScript frontend, and a .NET service.

---

## Incremental checking gives ratcheting for free

D8 in [12-known-limitations.md](12-known-limitations.md): teams adopt this kind of tool on codebases
that already violate the rules, and without a baseline the first run is a wall of errors and the
ruleset gets deleted.

**Checking only changed files solves that without a baseline file.** A repository with two hundred
existing violations passes every commit that does not add a two-hundred-and-first. The ratchet is
implicit in the scope, needs no state file, cannot drift from reality, and never has to be
regenerated after a refactor.

A baseline file is still worth having eventually — for the CI job that checks the whole repository —
but it stops being a prerequisite for adoption. That reordering matters: it moves the rules feature
from "usable on greenfield" to "adoptable anywhere" for far less work than D8 implies.

---

## Interface

```
tropism check [FILES...]        # check the given files against the rules
tropism check --staged          # files staged in git
tropism check --since <ref>     # files changed since a ref, for CI on a PR
tropism check                   # whole repository
```

`check` runs **only the rule checks** — `module-rule` and `package-rule`. That is deliberate and is
the division [10-js-evaluation.md](10-js-evaluation.md) argues for: the sound checks gate, the
advisory ones inform. `tropism analyze` remains the full run.

### A file list is the primary interface, not git

`tropism check src/a.rs src/b.ts` takes a plain list of paths. Everything else is sugar over it.

This is not incidental. Every pre-commit framework already passes the changed files as arguments —
[pre-commit](https://pre-commit.com), lefthook, husky + lint-staged all work this way — so the
primary interface is the one the ecosystem already speaks. It also keeps tropism free of a git
dependency: no subprocess, nothing to install, and the tool still works on a directory that is not a
repository at all.

`--staged` and `--since` are conveniences. Implement them by reading the git index directly with
`gix` (pure Rust) rather than shelling out to `git`, so a bare checkout with no git binary still
works. If that proves awkward, they can be dropped without loss — the file-list form is what the
hooks actually use.

### Distribution, and what prek actually does

[prek](https://github.com/j178/prek) is a Rust reimplementation of pre-commit — a single binary with
no Python runtime, config-compatible with pre-commit, and already used by CPython, Airflow, and
FastAPI. It is the right target: a hook framework that needs no runtime, for a hook that needs no
runtime.

Verified against prek's documentation on 2026-07-31 (prek v0.4.11):

> prek installs binaries via `cargo install --bins --locked` and runs the specified executable. The
> repository should contain a `Cargo.toml` that produces the binary referenced by `entry`.
> `additional_dependencies` and `language_version` are supported.

That gives three possible integrations, and **the obvious one does not currently work**:

| Route                                                  | Mechanism                                            | Status                                 |
| ------------------------------------------------------ | ---------------------------------------------------- | -------------------------------------- |
| `language: system`                                     | user installs `tropism` first                        | works as soon as binaries are released |
| `language: rust` on this repo                          | prek runs `cargo install --bins --locked` on a clone | **blocked** — see below                |
| `repo: local` + `additional_dependencies: ["tropism"]` | prek installs from crates.io                         | works once published                   |

**`language: rust` is blocked by the workspace layout.** The repository root is a virtual manifest,
and `cargo install --path .` refuses it:

```
error: found a virtual manifest at `/…/Cargo.toml` instead of a package manifest
```

Enabling it means making the root an installable package — moving the CLI crate to the repository
root, in the layout ripgrep uses. That is a real restructure with knock-on effects on
`tropism.toml`'s module globs and on the crate layout in
[01-architecture.md](01-architecture.md), so it is a decision rather than a detail. Recorded as D25
in [12-known-limitations.md](12-known-limitations.md).

Note the useful consequence of prek's `--locked`: it installs the exact versions from `Cargo.lock`,
which is why that file is committed ([.gitignore](../.gitignore) says so explicitly).

### Not shipped yet, and why

`.pre-commit-hooks.yaml` is still deliberately absent. The hook's `entry` is `tropism check`, and
that subcommand does not exist — D24 in [12-known-limitations.md](12-known-limitations.md). Shipping
a hooks file whose entry point is missing would advertise something broken to every repository that
consumed it.

The order is therefore: `tropism check [FILES...]`, then binaries, then the hooks file.

### What *is* wired up: a local hook on this repository

[`prek.toml`](../prek.toml) carries a `repo: local` hook that runs
`tropism analyze . --format text --fail-on error` over the whole tree. It is the same gate the CI
`dogfood` job applies, so a commit that would fail there fails at commit time instead.

Three deviations from the design above, each with a reason worth keeping in view:

- **`language: system`, not prek's `language: rust`.** The `rust` integration installs with
  `cargo install --bins --locked`, which refuses this repository's virtual manifest (D25). The hook
  runs `cargo run` against the local workspace instead. That is fine for a repo whose developers
  necessarily have cargo, and it is not a distributable arrangement.
- **Whole repository, not changed files.** This is the slow hook this document warns against,
  accepted knowingly because at ~0.6 s here the warning has no teeth yet. It will acquire teeth on a
  large repository, and the fix is `tropism check`, not a faster whole-repo run.
- **`--format text` explicitly.** prek captures stdout, so `--format auto` sees a non-tty and emits
  JSON. The whole value at commit time is the ruleset's `reason` rendered in a diagnostic, and JSON
  destroys it.

The hook was verified in both directions: it passes on a clean tree, and blocks with readable
diagnostics when a rule is violated.

---

## What "checking a change" actually means

A rule violation is an edge between two modules. An edge has two ends, so "did this change introduce
a violation?" is not simply "does this file violate a rule".

**Report a violation when the *source* end of the edge is a changed file.** If `src/api/user.ts`
gains `import { db } from '../data/db'`, the edge `api → data` is attributable to that file and is
reported. If `data/db.ts` changed and something in `api` already imported it, the edge is not new and
is not reported.

This is the honest reading of "what did this commit do", and it is what makes the ratchet work.

### The passes that are still needed

Only *parsing* is incremental. Two things must still be known globally, and both are cheap:

| Needed                        | Cost                      | Why                                           |
| ----------------------------- | ------------------------- | --------------------------------------------- |
| Project roots                 | one tree walk, no parsing | to know which project a file belongs to       |
| Module map (globs → modules)  | reading `tropism.toml`    | to know which module a path is in             |
| Manifests of touched projects | small files               | declared edges are rule-checkable too         |
| Module → file map             | one tree walk, no parsing | to resolve an *internal* import to its target |

The expensive stage — tree-sitter parsing every source file — shrinks to the changed set. That is the
win, and it is a large one: parsing dominates the 1.3 s Prometheus figure.

### The case that needs care

Resolving an internal import needs to know which file defines the target module, and for C# that
currently comes from parsing every file's `namespace` declaration
([csharp.rs](../crates/tropism-lang/src/csharp.rs)). A changed-files-only run does not have that.

Two options, and the choice should be measured rather than assumed:

1. **Cheap scan for module identity.** Read every file but extract only the module declaration — a
   line scan, not a parse. Cheap in absolute terms and correct.
2. **Cache the module map**, keyed on file content hashes, as
   [01-architecture.md](01-architecture.md) already contemplates for extraction.

Start with (1). It is simple, has no invalidation problem, and if a full cheap scan of a large
repository turns out to cost more than a few tens of milliseconds, (2) is the fallback.

---

## Output

The hook case wants terse output. The default `--format text` diagnostics are already right — the
`reason` from the ruleset is exactly what a developer needs at the moment of being blocked, since it
explains *why* the constraint exists rather than merely that it was broken.

Two additions:

**Say what was skipped.** A run that checked six files must not look like a run that checked the
repository. Consistent with `CheckStatus` and with exclusion disclosure:

```
checked 6 changed file(s) against 4 rules — 1 violation
  (pre-existing violations elsewhere are not shown; run `tropism check` for the full repository)
```

**Count, do not hide, the pre-existing violations.** If a full run would report twelve and the
changed files account for one, say so. A ratchet that silently conceals the backlog is how a
codebase ends up with two hundred violations nobody remembers agreeing to.

Exit codes are unchanged: `0` clean, `1` violations at or above `--fail-on`, `2` could not run.

---

## Open questions

1. **Should `cycle` run in the hook?** It is sound, and a newly-introduced cycle is exactly what you
   want caught at commit time. But cycle detection needs the whole graph, so it cannot be scoped to
   changed files the way rules can. Probably: not in the hook, yes in CI.
2. **Renames and deletions.** A file that moved between modules can create a violation without its
   contents changing. Checking the source end catches the new path, so this may already work — needs
   a test rather than a decision.
3. **What about a changed `tropism.toml`?** Editing the ruleset can invalidate the whole repository,
   so a change to it should probably force a full run rather than an incremental one.
4. **Merge commits and rebases.** `--since` against the wrong base can produce a huge changed set.
   Document the base-ref choice rather than guessing it.

---

## Where this sits in the plan

Ahead of the MCP server, and ahead of more languages.

[09-product-review.md](09-product-review.md) concluded that tropism cannot compete on detection
because native tooling already detects and *fixes*. The pre-commit hook is not a detection claim: it
is a claim about *when* and *how cheaply* the check runs, and on that axis the hermetic design wins
outright. It is also the shortest path from "tool that exists" to "tool that is in someone's
workflow every day", which is the thing the project has never had.

It depends on the release pipeline ([13-build-and-release.md](13-build-and-release.md)) — a hook
needs an installable binary — so those two ship together or not at all.
