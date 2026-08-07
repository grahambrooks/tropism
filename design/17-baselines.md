# 17 — Baselines for whole-repository runs

**Not implemented.** This is D8 in [12-known-limitations.md](12-known-limitations.md), and it is the
next product step in [07-open-questions.md](07-open-questions.md)'s build order.

---

## What this is for, and what it is not for

[14-incremental-checking.md](14-incremental-checking.md) already solved adoption. A run scoped to
changed files passes on a repository with two hundred existing violations as long as the commit does
not add a two-hundred-and-first, and it does that **with no state file, no drift, and nothing to
regenerate after a refactor**. That is a better ratchet than a baseline, and it is why D8 was
downgraded rather than built.

So this document has to justify a baseline against that, not assume one is wanted.

The gap is narrow and real: **the CI job that checks the whole repository.**

| Surface | Ratchet today | Gap |
| --- | --- | --- |
| `tropism check <files>` — pre-commit | scope | none |
| `tropism check --since <ref>` — CI on a PR | scope | none |
| `tropism analyze .` — CI on main, nightly, release gate | **none** | wall of errors, or `--fail-on` turned up until it means nothing |

The third row is the whole of D8. A team that adopts the hook gets a working ratchet immediately and
then discovers their whole-repository job cannot be turned on at all. The two realistic responses
today are to not run it, or to raise `--fail-on` to `error` and then find rules default to `error`
anyway — so in practice, to not run it.

**A baseline is therefore a CI feature, not an adoption feature.** That framing matters because it
sets the bar: it does not have to be the primary ratchet, so it can afford to be conservative and
loud where the scoped ratchet is silent and automatic.

---

## The problem a baseline has that scope does not

Scope-based ratcheting cannot drift, because it holds no state. A baseline is a file that describes
findings, and findings move. This is the entire difficulty, and any design that does not confront it
directly is just deferring a wall of errors to the day someone runs a rename.

Consider what `finding_id` currently keys on
([report.rs](../crates/tropism-core/src/report.rs)):

```
finding_id(check, project, key_parts) = "check:project:" + blake3(check, project, ...key_parts)[..8]
```

and for a module-rule violation the key parts are
`[rule_id, from_module, to_module, source_file]`
([rules.rs](../crates/tropism-core/src/rules.rs), `module_finding`).

The source path is *in the hash*. So:

| Change | ID | Baseline keyed on ID |
| --- | --- | --- |
| Fix one of five violations | other four unchanged | ✅ still suppressed |
| Add a sixth violation | new | ✅ reported — the ratchet works |
| **Rename `api/user.ts` → `api/account.ts`** | **new** | ❌ **reported as new, and the old entry goes stale** |
| Move `api/user.ts` → `data/user.ts` | new | reported as new |

Row three is the killer. A pure rename is not an architectural change, and a baseline that turns
`git mv` into a failing build is a baseline that gets deleted — the same death as the wall of errors
it was meant to prevent.

---

## The central idea: match on what the rule is about

A rule violation is **an edge between two modules**. That is the tool's own model, stated in
[11-dependency-rules.md](11-dependency-rules.md) and reused for scoping in
[14-incremental-checking.md](14-incremental-checking.md). The file is *where* the edge was found; the
modules are *what the rule is about*.

So the matching rule falls out of what the tool is for:

> **A change that keeps a violation inside the same module pair is not an architectural change, and
> the baseline should survive it. A change that moves it to a different module pair is exactly what
> the rules exist to detect, and the baseline must not survive it.**

Two-level matching, in order:

1. **Exact** — the finding ID. Unambiguous, and covers the common case where nothing moved.
2. **Structural** — the tuple `(rule_id, from_module, to_module)`, with an **occurrence count**.

A rename hits (2): the module pair is unchanged, the count is unchanged, the violation stays
baselined. Adding a violation raises the count above the baselined figure, and the excess is
reported. Moving code across a module boundary changes the pair, so it does not match at all and is
reported in full.

**The count is what stops (2) becoming a blanket amnesty.** Without it, one baselined `api → data`
edge would suppress fifty new ones. With it, the baseline says "twelve of these were here when we
started" and the thirteenth fails the build.

### Which one is reported when the count is exceeded

The excess findings are the ones whose IDs are *not* in the baseline, preferred in that order. When
that does not fully disambiguate — twelve baselined, thirteen found, none matching by ID because
every file was renamed at once — report the excess without claiming to know which one is new, and
say so:

```
3 of 13 `api-goes-through-the-domain` violations exceed the baselined count of 10.
The baseline could not identify which: every source file has moved since it was written.
Regenerate with `tropism baseline --write` after reviewing.
```

Guessing which specific violation is new would be a confident wrong answer about a line of source,
which is the one thing this codebase consistently refuses to do.

---

## The file

`tropism-baseline.toml`, beside `tropism.toml` at the scan root.

**A separate file, not a section in `tropism.toml`, and there is a concrete reason beyond taste.**
Open question 3 in [14-incremental-checking.md](14-incremental-checking.md) is already decided: a
change to `tropism.toml` **widens `tropism check` to the whole repository**, because editing a rule
can invalidate anything. If the baseline lived in that file, every baseline regeneration would force
a full-repository run on the next commit — turning the cheap path expensive exactly when someone is
doing routine maintenance. The two files also have opposite natures: `tropism.toml` is hand-written,
small, and reviewed line by line; a baseline is generated, potentially long, and reviewed in bulk.

Sketch:

```toml
schema_version = 1

# Written by `tropism baseline --write` on 2026-08-06 against 4f2a1c9.
# Every entry here is a violation the team has accepted for now. Nothing in this
# file is a rule; deleting an entry can only make the build stricter.

[[entries]]
rule = "api-goes-through-the-domain"
from = "api"
to = "store"
count = 12
ids = [
  "module-rule:.:a3f81b02",
  "module-rule:.:c7d24e91",
  # ...
]
note = "Legacy checkout flow. Tracked in PLAT-284."
```

`ids` carries level (1) and `count` carries level (2). Both are written; either can match.

**Canonically ordered, like every other output.** Same input must produce the same bytes, or the file
churns in every diff and nobody reads it.

---

## Baselined is not invisible

The doctrine here is settled by `CheckStatus` and by `exclude`: a deliberate blind spot is always
disclosed with a count, because **a silent blind spot is indistinguishable from a clean result.** The
workspace exemptions added for open question 1 follow the same rule.

A baseline is a much larger blind spot than either. So:

- A baselined finding is **downgraded, not deleted**. It stays in the JSON report with
  `baselined: true` and a severity of `info`, so it cannot set the exit code but can still be listed,
  counted, and graphed over time.
- Every report ends with the count, unconditionally: `47 finding(s) baselined by
  tropism-baseline.toml`. Not behind a flag. A team whose baseline is growing should be unable to
  avoid noticing.
- `--no-baseline` shows the unfiltered truth, and CI dashboards should use it.

This is the difference between a baseline and an exclusion, and it should be stated in the file's own
header comment: **an exclusion means "never look here"; a baseline means "we looked, we know, not
today".** The second is only honest if the number stays visible.

---

## Staleness, which is the same failure as everywhere else

A stale rule is reported ([rules.rs](../crates/tropism-core/src/rules.rs)) because a rule that checks
nothing protects nothing. A stale `exclude` pattern is reported with `(matches nothing)` for the same
reason. A stale baseline entry is worse than either: it is a *fixed* violation still holding a
suppression open, so the next regression in the same place is absorbed silently.

So: **an entry matching nothing is reported**, at `info`, with the suggestion to regenerate.

The pleasant consequence is that the baseline shrinks visibly as the backlog is paid down, which is
the one thing that makes a team want to pay it down.

---

## Generating it

```sh
tropism baseline --write        # create or replace, from the current findings
tropism baseline --prune        # drop only the entries that no longer match
tropism baseline                # show what would change, write nothing
```

Three rules:

- **Never automatic.** No `--update-baseline-on-failure`, no writing as a side effect of `analyze`. A
  baseline that regenerates itself is not a ratchet, it is an off switch with extra steps.
- **`--prune` is the routine operation** and can be safe to run in CI on a schedule; `--write` is the
  one that needs review, because it accepts everything currently broken.
- The file records **when and against which commit** it was written, in a comment. A baseline whose
  provenance is unknown cannot be audited.

---

## What may be baselined

**Only checks that gate.** In practice that is `module-rule` and `package-rule`, which default to
`error` because the team asserted them.

Baselining `unused-dep` would be baselining a check measured at **63% false positives**
([10-js-evaluation.md](10-js-evaluation.md)) and which already never gates. The entries would be
mostly wrong, and reviewing them would teach the team that baseline entries are noise — which
destroys the file's value for the checks where it matters. `cycle` is sound but advisory and
whole-graph; if it is ever promoted to gating, revisit.

Recommendation: the baseline applies to `CheckId::RULES` only, and `tropism baseline --write` refuses
to record anything else, saying why.

---

## Does `check` honour the baseline?

**Yes**, and it is worth being explicit because the answer is not obvious.

`check <files>` already ratchets by scope, so most of the time the baseline is irrelevant to it. The
case where it is not: someone edits a file that carries a pre-existing violation. The edge's source
end is now a changed file, so `check` reports it, and the commit is blocked by something the author
did not do.

That is the "boy scout tax", and it is how a hook gets uninstalled. Honouring the baseline in `check`
removes it.

The counter-argument — *you are in that file anyway, fix it* — is real but is a team's call, not
tropism's. If it is wanted, it is `--no-baseline` in the hook config, which is one line and explicit.

---

## Open questions

1. **Expiry dates.** `expires = "2026-12-01"` would stop a baseline becoming permanent, which is its
   most likely failure mode. But tropism currently has **no clock dependency at all**, and every
   check is a pure function of the files in the repository. Introducing "the answer depends on what
   day it is" is a real cost to a tool whose reproducibility is a selling point. Probably: not in v1,
   and revisit if real baselines are seen going stale.
2. **Per-directory baselines in a monorepo.** Same shape as D9 and should get the same answer
   whenever D9 gets one; do not decide it separately.
3. **Does the structural fallback need the file's module rather than just the module pair?** Matching
   on `(rule, from, to)` treats all twelve violations of one rule as interchangeable. That is
   probably right — they *are* the same architectural fact — but it means a fix and a regression in
   the same module pair cancel out invisibly. Measure against a real backlog before adding a third
   key level.
4. **Should the baseline be readable by the MCP server?** An agent about to edit legacy code would
   benefit from knowing a violation is already accepted. Cheap, but it waits on the MCP server.

---

## Where this sits in the plan

Third in the build order in [07-open-questions.md](07-open-questions.md), after `tropism check` and
the release pipeline, both of which have shipped. It is ahead of the unimplemented rule kinds and
ahead of MCP.

It is the last piece of *"one ruleset, enforced at commit time and over the whole repository"* — the
commit-time half is done and the whole-repository half currently cannot be switched on by anyone with
a backlog, which is everyone the feature is for.
