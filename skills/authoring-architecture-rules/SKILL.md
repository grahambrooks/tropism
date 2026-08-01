---
name: authoring-architecture-rules
description: Write or repair a tropism.toml ruleset that encodes a repository's intended architecture — module boundaries, layering, and package policy — derived from what the code actually contains rather than from a template. Use this whenever someone wants to enforce architectural boundaries, stop layers reaching past each other, ban or scope a dependency, prevent modules becoming entangled, add tropism to a project, or fix a ruleset that errors or reports rules as stale. Also use it when someone describes an architecture rule in prose ("the API shouldn't touch the database directly", "keep the UI framework out of the domain", "these two services must stay independent") and wants it enforced, even if they never mention tropism or tropism.toml by name.
---

# Authoring a tropism ruleset

A ruleset says what may depend on what. tropism then enforces it across every language in the
repository, at commit time and in CI, without building anything or installing dependencies.

The value is not the file — it is that a constraint which previously lived in someone's head, or in
a wiki page nobody reads, becomes a thing that fails a build with the reason attached.

## The one mistake that makes rulesets useless

**A rule written from a template is worse than no rule.** It either matches nothing (and silently
protects nothing), or it matches everything (and gets deleted the first time it blocks someone).

So the order of work is: **read the repository first, propose rules second.** Every module name in
the ruleset must correspond to directories that exist, and every rule must describe a boundary the
team actually cares about. If you cannot name the consequence of a boundary being crossed, do not
write the rule.

## Workflow

### 1. See what is there

```sh
tropism analyze . --format json
```

This lists every project, its language, and the checks that ran. It works on a fresh checkout with
nothing installed, so there is no setup step to get through first.

Read the output for the *shape* of the repository: how many projects, which languages, whether it is
a monorepo of packages or one tree with directories. Then look at the actual layout — the directory
names are what module globs have to match.

### 2. Find the boundaries that already exist

Rules should describe the architecture the code already has, or is trying to have. Look for:

- **Layers.** Directories named `api`, `handlers`, `domain`, `core`, `store`, `db`, `repository`,
  `infra`. A layered design usually wants each layer to depend downward only.
- **Entry points.** `main.*`, `cmd/`, `bin/`, an executable target. These compose; they should not
  reach past their collaborators into the bottom of the stack.
- **Independent surfaces.** A CLI and a server, two services, a library and its example app. If two
  things are meant to be swappable or separately deployable, they must not import each other.
- **Something that leaked.** A UI framework imported in the domain, a SQL helper above the storage
  layer, a test framework referenced from production code. These make the best first rules because
  the team already knows they are wrong.

Ask the person you are working with when the intent is genuinely ambiguous. "Is `internal/` meant to
be reachable from `api/`?" is a better question than a guessed rule.

### 3. Write the modules, then the rules

Modules are named globs over paths. Rules reference modules by name.

```toml
schema_version = 1

[modules]
api   = "src/api/**"
core  = "src/core/**"
store = "src/store/**"
```

A module can name several globs when one concept spans them — a C++ component is a header and its
translation unit, and a rule about it should reach both:

```toml
[modules]
store = { paths = ["include/shop/store.hpp", "src/store.cpp"] }
```

Then the rules. There are exactly three module rule kinds and four package rule shapes — see
[references/rule-reference.md](references/rule-reference.md) for the complete syntax, including the
kinds that are *specified but not implemented* and will be rejected at parse time.

### 4. Give every rule a reason

```toml
[[module_rules]]
id = "api-goes-through-the-domain"
deny = { from = "api", to = ["store"] }
severity = "error"
reason = """
The API layer talks to the domain and nothing else. A controller that calls the
store directly couples HTTP concerns to the storage schema, and the domain stops
being the place where the rules live.
"""
```

`reason` is rendered verbatim in every finding, and it is the part no inferred check can supply. A
developer blocked at commit time gets *why the constraint exists*, not merely that it was broken.

Write it for the person who will be blocked by it in eight months and was not in the room. "Layering
violation" tells them nothing. The sentence above tells them what breaks and why someone cared.

### 5. Prove each rule can fail

A ruleset that parses is not a ruleset that works, and **a rule that passes and a rule that is
inert look identical in a clean report.** Only one of them protects anything. This is the step
people skip, and it is the step that finds the problems.

```sh
tropism check
```

**First, the cheap checks.** `rule X matched no dependency in this repository` is not a pass — it
means a glob is wrong or a directory was renamed, and the rule silently stopped protecting anything.
Fix the glob rather than deleting the rule. And read the summary line: `checked N file(s) against
**M rule(s)**`. If M is 0 or lower than you wrote, rules are not being evaluated.

**Then prove it.** Take a *copy* of the tree, introduce one deliberate violation per rule — the
exact import or declaration the rule exists to forbid — and confirm each one fires:

```sh
cp -r . /tmp/probe && cd /tmp/probe
# add `use forbidden_crate::Thing;` to a file in the restricted module
tropism check              # expect exit 1, naming that rule
```

Then throw the copy away. Never leave probes in the real tree.

This catches the failure that matters most: **a rule that does not fire on its own motivating
example.** It happens more than you would expect, and the usual cause is the next section.

### The name in a rule must be the name tropism *reports*

Package rules match reported package names exactly — no globbing, no fuzzy matching — and the
reported name is not always the name in the manifest.

The clearest case is Rust: a crate declared as `tree-sitter-go` is imported as `tree_sitter_go`, and
tropism normalises the underscore form back to the hyphenated one **only when that project's own
manifest declares the crate**. In a crate that has not declared it — which is exactly the
half-finished state a pre-commit hook sees, an import added before the manifest catches up — the
reported name is the underscore form, and a rule listing only `tree-sitter-go` does not match.

So for hyphenated packages, list both spellings and leave a comment saying why:

```toml
[[package_rules]]
id = "grammars-stay-in-the-providers"
# Both spellings: an import in a crate that has not yet declared the dependency
# is reported with underscores. Probing is what surfaces this.
packages = ["tree-sitter-go", "tree_sitter_go"]
allowed_in = ["providers"]
```

The general lesson is not about Rust: **verify by probing rather than by reading**, because the
name a rule must match is a fact about tropism's output, not about the manifest.

### 6. Adopt without a wall of errors

Existing codebases violate their own architecture. That is normal and is not a reason to weaken the
rules.

`tropism check <files>` reports only violations that the given files *introduce*, because a violation
is an edge and it belongs to the file at its source end. So a repository with two hundred existing
violations passes every commit that does not add a two-hundred-and-first — a ratchet with no baseline
file to maintain. Wire it as a pre-commit hook and the backlog stops growing from day one, while
`tropism check` with no arguments still shows the whole picture.

## Choosing severity

| Severity | Use for |
| --- | --- |
| `error` | Boundaries the team has genuinely agreed on. Blocks CI and commits. |
| `warning` | A rule being adopted against an existing backlog, or a boundary that is a strong preference. |
| `info` | Documentation of intent — visible in reports, blocks nothing. |

Rule findings default to `error` and to `High` confidence, because the team asserted them rather than
tropism inferring them. That is the whole reason rules are trustworthy enough to gate on when the
inferred checks are not.

## What not to do

**Do not write rules for things tropism infers anyway.** Cycles are already detected. A rule saying
"a must not depend on b" *and* "b must not depend on a" is a worse cycle check.

**Do not use `exclude` to make a failing gate pass.** `exclude` globs are for deliberately-broken
sample projects and vendored trees. Every exclusion is disclosed in the report with a match count,
because a silent blind spot is the failure this tool exists to prevent. Widening one to get green is
how a ruleset becomes decorative.

**Do not encode a boundary nobody agreed to.** A rule you invented will be deleted by the first
person it blocks, and it takes the credibility of the other rules with it.
