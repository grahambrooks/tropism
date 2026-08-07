---
name: tropism-in-the-loop
description: Use tropism to check architectural boundaries while writing code — before adding an import, after a refactor, or before finishing a task — and interpret its findings correctly, including which checks are trustworthy enough to act on and which are advisory. Use this whenever working in a repository that has a tropism.toml, whenever about to add a dependency or an import that crosses a module boundary, whenever a tropism check or pre-commit hook fails and the finding needs acting on, and whenever a refactor moves files between modules. Also use it when asked to keep a change within the architecture, to check whether something is allowed to import something else, or when a build fails with a module-rule or package-rule finding.
---

# tropism in the loop

tropism answers one question fast and hermetically: **does this change cross a boundary the team
said it should not?** It needs no build, no install, and no network, so it can run in the second
before you write an import rather than in CI twenty minutes later.

The point of using it while coding is not to run a linter at the end. It is to ask *before* writing
code that will be rejected, because the ruleset carries the team's `reason` and that reason usually
tells you what to write instead.

## Ask before, not only after

The highest-value moment is when you are about to add an import that crosses a boundary — a new
`use`, `import`, `require`, or `#include` reaching into another part of the tree, or a new entry in
a manifest.

```sh
tropism check path/to/file/you/are/editing.rs
```

If that reports a violation, **read the `reason`**. It states what the boundary is for. Usually the
right response is not to route around the check but to do the thing the reason implies:

- reaching past a layer → go through the layer, or move the logic down
- a package used where it is not allowed → put the call behind the module that owns that concern
- two modules that must stay independent → the shared piece belongs in something both may depend on

If the reason makes no sense for what you are doing, that is a signal worth raising with a human —
either the rule is wrong or the design has changed. Say so rather than silently working around it.

## Check what you changed, not the whole repository

```sh
tropism check src/a.rs src/b.ts        # the files you touched
tropism check --staged                 # what is staged
tropism check --since origin/main      # what this branch introduced
tropism check                          # everything (slower, for a full picture)
```

Scoping matters for a reason worth understanding: a violation is an **edge**, an edge has two ends,
and it belongs to the file at its *source* end. If `api/user.ts` gains an import of `data/db`, that
is attributable to `api/user.ts`. If `data/db.ts` changed and something in `api` already imported it,
the edge is not new and is not your change's fault.

So a scoped check tells you what *you* broke, separately from what was already broken. Most
repositories have a backlog; the scoped run is what stops you being blamed for it — and what stops
you adding to it.

The output states the backlog rather than hiding it:

```
checked 3 changed file(s) against 6 rule(s) — 0 violation(s)
  12 pre-existing violation(s) elsewhere are not shown; run `tropism check` for the whole repository
```

Zero violations there means *your change is clean*, not that the repository is.

## Which findings to act on

This is the part that matters most, and getting it wrong in either direction wastes everyone's time.

| Check | Trust | What to do |
| --- | --- | --- |
| `module-rule`, `package-rule` | **High.** The team asserted it; a violation is a line of source. | Fix it, or raise the rule with a human. Never work around it silently. |
| `cycle` | **High.** Reads only import syntax. | A newly-introduced cycle is worth fixing now. A pre-existing one is a design conversation. |
| `version-conflict`, `diamond-dep` | Sound **about the lockfile**, which is not the same as about your build. | Investigate; do not treat as a defect on sight. See below. |
| `missing-dep` | Good, capped at Medium confidence. | Usually real. Check before adding a declaration. |
| `unused-dep` | **Weak — 63% false positives measured on real repositories.** | Do not remove a dependency because of this alone. Verify by hand. |

`unused-dep` is unreliable for a structural reason: packages are legitimately used through channels
a hermetic tool cannot see — HTML `<script src>`, config files, framework strings, CLI arguments in
scripts. Deleting a dependency on its say-so breaks builds. Treat it as a hint to investigate, never
as an instruction.

`version-conflict` and `diamond-dep` describe the **lockfile**. A lockfile is resolved once for every
feature combination and every target platform and records neither, so it can list copies that no
build ever compiles — a package pulled in only by a disabled optional feature, or by another
platform. Before acting, confirm the duplicate is real for your build (`cargo tree --duplicates`,
`npm ls <pkg>`).

## A green check is not proof the rules ran

Read the summary line, not just the exit code:

```
checked 6 changed file(s) against 4 rule(s) — 0 violation(s)
```

**The rule count is the part that matters.** If it says `against 0 rule(s)`, nothing was enforced —
and `tropism check` still exits 0 in that case, so a hook or CI step passes while protecting
nothing. The commonest cause is a `tropism.toml` that fails to parse: the whole file is rejected, so
one bad rule disarms every other rule in it, including the correctly-written ones.

tropism does print `no rules were evaluated — nothing here can fail` when this happens, but a passing
exit code is what a hook acts on and a human skims past the warning. So when a check comes back
clean after any change to the ruleset, confirm the rule count is what you expect before believing
it. `tropism analyze . --fail-on error` does exit non-zero on an unparseable ruleset, which makes it
the more reliable gate for that specific failure.

## Zero findings is not the same as checked

Every check reports `ran`, `unavailable`, or `failed`, with a reason:

```
unavailable version-conflict — go.sum records hashes for the whole module graph, not the
                               versions MVS selected, and carries no edges; a resolved tree
                               needs the Go resolver
```

An empty finding list means nothing until you know which checks produced it. When reporting results
to a human, say what did not run and why — "no cycles found" is misleading if cycle detection was
unavailable. `tropism check` deliberately reports the inferred checks as not-run, because it
evaluates only the rules.

## When a finding — or its absence — does not make sense

Two questions come up constantly and both have a command rather than a guess.

**"Why was this import classified that way?"**

```sh
tropism explain path/to/file.ts
tropism explain path/to/file.ts --import lodash
```

Every import in the file, the name it resolved to, and one sentence saying why. Use it before
concluding that tropism is wrong about a dependency — most surprises are an `unresolved` import
(which contributes no edge, so no rule can match it) or a package name that differs from the
manifest spelling.

**"Why is this undeclared import not reported?"**

```sh
tropism workspaces .
```

Projects in the same workspace may import each other's published names without declaring them, so
`missing-dep` deliberately passes over those. Every such exemption is disclosed in the report with
the package, the project that supplied it, and the number of import sites — it is never silent.

What to look at: any workspace whose origin is `language`. That is an inference tropism made because
the ecosystem declares no workspace at all (Python, Ruby, Swift, C++, NuGet), and if it has grouped
independent services together, genuine missing dependencies between them go unreported. The fix is
`[[workspaces]]` in `tropism.toml`, not a change to the code.

Do not treat a crossing as a bug in tropism either. An import satisfied by another workspace resolves
today through hoisting and breaks when the package is built alone — it is a real defect, and a
`crosses_workspace` rule is how a team makes it an error.

## Reading the output

`--format json` when you need to process results; `--format text` when a human will read them,
because the diagnostics include the source line and the ruleset's `reason`. Piped output defaults to
JSON, which is why an agent capturing stdout gets JSON unless it asks for text.

Exit codes: `0` clean, `1` findings at or above `--fail-on`, `2` could not run. A `2` is a broken
invocation — a missing path, an unparseable ruleset — and must never be read as a pass.

## When there is no ruleset

If `tropism.toml` does not exist, the rule checks report `unavailable`, not clean. tropism will still
find cycles and manifest problems, but the highest-value capability is inert.

That is worth saying to the user once: the rules are the part no other tool can do, because
`go mod tidy` and its equivalents cannot know an architecture nobody told them about. Offer to write
one — there is a companion skill for authoring rulesets — rather than proceeding as if the tool were
fully armed.

## Finishing a task

Before declaring work done in a repository with a ruleset:

```sh
tropism check --since <base-branch>
```

This reports what the whole change introduced, which is what a reviewer will see. Fix violations
attributable to your work. Do not fix the pre-existing backlog as a side effect of an unrelated
task — that makes the diff unreviewable — but do mention it if it is large.
