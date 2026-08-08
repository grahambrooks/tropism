# 18 — Rego as a second rule language

**Evaluated 2026-08-08, prototyped, not adopted.** The question was whether
[Rego](https://www.openpolicyagent.org/docs/policy-language) could express tropism's architecture
rules *alongside* `tropism.toml` rather than replacing it.

Everything below was measured against a working prototype using `regorus` 0.11.0 — Microsoft's Rust
implementation of Rego — fed a tropism-shaped input document. Nothing here is inferred from the
language specification.

---

## The short answer

**Technically viable, and more so than expected. Not worth building yet.**

Every hazard turned out to be mitigable, and the one property I expected to lose outright —
stale-rule detection — has a partial analogue. What sinks it for now is not feasibility but
priority: the only thing Rego buys that tropism cannot already do is *expressiveness*, and
expressiveness is not the axis this product competes on.

---

## What was prototyped

The input document hands Rego edges **already resolved to modules**, so the glob dialect stays in
`tropism.toml` and policies talk about architecture rather than paths:

```json
{ "edges": [ { "from_module": "cli", "to_module": "mcp",
               "file": "crates/tropism/src/main.rs", "line": 21,
               "level": "imported", "label": "tropism_mcp::serve" } ],
  "packages": [ { "name": "reqwest", "at": "crates/tropism-core/Cargo.toml",
                  "line": 22, "level": "declared" } ] }
```

Three of this repository's own rules were written as Rego and all three fired correctly, including
one **`tropism.toml` cannot express today**:

```rego
# "No network client below the edge" — a package rule scoped by path, inverted.
deny contains finding if {
    some use in input.packages
    use.name in {"reqwest", "hyper", "ureq"}
    startswith(use.at, "crates/tropism-core/")
    finding := { "rule": "core-makes-no-network-calls", "file": use.at, "line": use.line,
                 "message": sprintf("`%s` is a network client and core must stay hermetic", [use.name]) }
}
```

## What it costs, measured

| | Result |
| --- | --- |
| Evaluation, 1,000 edges | 1.3 ms |
| Evaluation, 10,000 edges | 14 ms |
| Evaluation, 50,000 edges | 60 ms (load 52 ms) |
| Binary size | **+2.0 MB** (460 KB → 2.5 MB on a minimal harness) |
| Transitive crates | **+20**, including `num-bigint`, `parking_lot`, `rand`, `getrandom` |

Performance is a non-issue: linear, and `check` sees only the changed files' edges anyway. The
binary cost is not nothing for a tool that cross-compiles to six targets and is about to be code
signed, but it is affordable.

## The three hazards, and that all three are mitigable

**1. Hermeticity — solved by feature flags.** With `default-features = false` and only
`std, regex, semver`, the dangerous builtins are gone. Probed directly:

```
http.send            EVAL-REJECTED      regex.match     EVALUATED
net.lookup_ip_addr   EVAL-REJECTED      semver.compare  EVALUATED
time.now_ns          EVAL-REJECTED      sprintf         EVALUATED
uuid.rfc4122         EVAL-REJECTED
opa.runtime          EVAL-REJECTED
```

The core constraint survives, and it survives *at compile time* rather than by policy review.

**2. Determinism — a real hole, mitigable by rejection.** `rand.intn` is gated on the **`std`**
feature, not on `rand`, so no practical build can exclude it. It returned 30, 601 and 378 in three
separate processes — and 192, 830, 489 within a *single* process. tropism publishes "same input, same
bytes"; a policy calling `rand.intn` breaks it.

The mitigation matches doctrine this project already follows. `get_ast_as_json()` exposes the
builtin **before evaluation**, so a policy using a non-deterministic builtin can be rejected at load
time with an error naming it — exactly how `layers`, `require` and `transitive` are handled in
`tropism.toml` today, and for the same reason: a ruleset must never appear to enforce more than it
does.

**3. Runaway policies — bounded.** Rego is not total; a policy can burn arbitrary CPU. A 20-million
element comprehension aborted cleanly under `set_execution_timer_config`:

```
runaway with 200ms limit -> ABORTED: execution exceeded time limit after 295ms
```

That matters more here than in most OPA deployments, because the thing being blocked is somebody's
commit.

## Stale-rule detection: weaker, not absent

This was the expected deal-breaker. "A rule that checks nothing protects nothing" is a defining
property, and for an arbitrary Rego policy "produced no findings" is not obviously distinguishable
from "satisfied".

**Coverage gets most of the way there.** With `set_enable_coverage(true)`, a stale rule (module
renamed) and a *typo'd* rule (`from_moduel`) both surfaced as uncovered, while a live rule did not:

```
NOT COVERED 11: deny contains f if {              # legacy-is-frozen — module renamed
NOT COVERED 14: f := {"rule": "legacy-is-frozen"} #
NOT COVERED 18: deny contains f if {              # core-is-a-leaf — `from_moduel` typo
NOT COVERED 21: f := {"rule": "core-is-a-leaf"}   #
```

Catching the typo is the more valuable half. A misspelled field in Rego evaluates to `undefined`,
matches nothing, and passes silently — precisely the failure mode this project is organised against,
and `tropism.toml` catches its equivalent by rejecting unknown module names at parse time.

**But it is a weaker signal, and the difference must not be glossed.** A *satisfied* rule also stops
short of its assignment. The two are distinguishable only by *how deep* coverage reaches — a
satisfied rule evaluates all its guards, a stale one stops at the guard that never matched — and
that distinction degrades to nothing for a single-guard rule. `tropism.toml`'s staleness test is
exact, because its vocabulary is closed: it asks whether a module glob matches any path, which is
answerable without evaluating anything.

The fix, if this is ever built: **keep the module vocabulary in `tropism.toml`** and have policies
reference those names. tropism then keeps its exact staleness test, and Rego is confined to
expressing relationships between names tropism already validates.

## The tension nobody would find by reading the docs

**D36 makes the input document partial, and Rego policies are not obliged to be per-edge.**

`tropism check` parses only the changed files, so `input.edges` under the hook is a *subset*. Every
built-in rule is a predicate over one edge, so a subset is safe — that is exactly what makes the
ratchet honest. An aggregate policy is not:

```rego
# Looks reasonable. Silently wrong under `tropism check`.
deny contains f if {
    count([e | some e in input.edges; e.to_module == "core"]) > 20
    f := {"rule": "core-has-too-many-dependents"}
}
```

Under `analyze` this is right; under `check` it sees a handful of edges and never fires. That is a
rule the hook cannot honestly evaluate, and
[14-incremental-checking.md](14-incremental-checking.md) is explicit that such a rule "would be worse
than no rule, because it would make the hook a liar."

Three ways out, none free: pass `input.scope` and make it the author's problem; refuse aggregate
policies (undecidable in general); or evaluate Rego only in `analyze` and tell the user the hook does
not run their policies. The last is the honest one and it is also an admission that the two rule
languages are not equivalent.

## Why not now

Rego's whole value here is expressiveness, and the concrete gap it would close is **D6 and D7** —
`layers`, `require`, `transitive`, and version constraints, all currently rejected at parse time.
Four well-understood features, each a few hundred lines, against a second rule language, +2 MB, +20
crates, a determinism hole needing a load-time guard, a weaker staleness signal, and a scope
asymmetry that has no clean answer. Implementing the four natively is less work and leaves the tool
with one vocabulary to document, one set of error messages, and one story in the skills.

It also does not advance the thesis. *"One ruleset, enforced at commit time and over the whole
repository, across ten languages, with no build and no install"* — Rego touches none of those
clauses, and weakens "one ruleset".

## What would change the answer

- **A team with an existing OPA deployment asks for it.** That is the real case: policy reuse,
  `opa test`, bundle distribution, and org-wide policy that spans more than architecture. Nobody has
  asked.
- **Rule kinds keep accumulating.** If the list of things teams want to express keeps growing past
  D6/D7, owning a rule kind per idea stops scaling and a general language starts paying for itself.
- **`regorus` reaching 1.0** would reduce the API risk, though 0.11 was stable enough to prototype
  against without trouble.

## If it is built, the shape is constrained

Not "arbitrary Rego". Every one of these came out of the prototype:

1. `default-features = false`, features `std, regex, semver` — no `http`, `net`, `time`, `uuid`.
2. Non-deterministic builtins **rejected at load** via `get_ast_as_json`, naming the builtin.
3. `set_execution_timer_config` always set; a hook must not hang.
4. Input is edges **already resolved to modules**; the glob dialect stays in `tropism.toml`.
5. Modules declared in `tropism.toml`, referenced by name from Rego, so exact staleness survives.
6. Every finding **must carry a file**, or `check` cannot scope it — that is what preserves the
   ratchet, since scoping already filters on the first evidence path.
7. Coverage reported alongside findings, as the closest available analogue to stale detection.
8. Findings default to `Warning`, not `Error`. A `tropism.toml` rule earns `High` confidence because
   a violation is the *presence* of an import — a fact about a line of source. An arbitrary Rego
   policy can assert anything, and inherits no such guarantee.

Point 8 is the one to argue about first, because it decides whether Rego rules can gate a commit at
all.
