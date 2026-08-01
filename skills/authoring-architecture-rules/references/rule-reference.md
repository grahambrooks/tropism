# tropism.toml — complete reference

Everything the parser accepts, and everything it rejects. Read the rejected list too: several rule
kinds are *specified* in tropism's design docs but not built, and using one is a parse error rather
than a silent no-op.

## Contents

- [Top level](#top-level)
- [Modules](#modules)
- [Module rules](#module-rules) — `deny`, `independent`, `allow_only`
- [Package rules](#package-rules) — denylist, scoping, closed-world
- [Exclusions](#exclusions)
- [Rejected at parse time](#rejected-at-parse-time)
- [Worked examples](#worked-examples)

## Top level

```toml
schema_version = 1        # required to be 1 if present
exclude = ["demo/**"]     # optional, applied before discovery
[modules]                 # named globs
[[module_rules]]          # zero or more
[packages]                # optional, closed-world switch
[[package_rules]]         # zero or more
```

Unknown keys are a parse error, deliberately: a typo that made a rule silently vanish would be the
worst possible failure for a tool whose value is that it catches things.

## Modules

A module is a name bound to one or more path globs. Names are referenced by rules; a rule naming an
undefined module is a parse error.

```toml
[modules]
api  = "src/api/**"                                     # one glob
core = { paths = ["src/core/**", "src/shared/**"] }     # several
```

When a path matches more than one module, **the longest pattern wins**. So a broad module can be
carved into a narrower one without ambiguity.

## Module rules

Every rule needs an `id`. `severity` defaults to `error`; `reason` is optional but is the most
valuable field in the file. Exactly one rule kind per rule — setting two is a parse error, with the
advice to split it in two.

### `deny` — a directed prohibition

```toml
[[module_rules]]
id = "api-goes-through-the-domain"
deny = { from = "api", to = ["store", "infra"] }
reason = "..."
```

`api` must not depend on `store` or `infra`. Says nothing about the reverse direction.

### `independent` — mutual prohibition

```toml
[[module_rules]]
id = "surfaces-are-independent"
independent = ["cli", "mcp"]
reason = "Both are adapters over one core. Shared behaviour belongs in core."
```

No member may depend on any other member. Use for things meant to be separately deployable or
swappable. Equivalent to a `deny` in every direction, but says the intent once.

### `allow_only` — a closed world for one module

```toml
[[module_rules]]
id = "entrypoint-composes-the-api"
allow_only = { from = "entry", to = ["api"] }
reason = "..."
```

`entry` may depend on `api` and on nothing else that the ruleset names. Stronger than `deny` and
better for entry points, because it stays correct when a new module is added later — `deny` would
need updating, `allow_only` would not.

An empty `to` means "may depend on nothing":

```toml
allow_only = { from = "core", to = [] }     # core is a leaf
```

## Package rules

About third-party dependencies rather than internal modules.

**The two shapes match at different levels, and the difference is deliberate:**

| Shape | Fires on a manifest declaration | Fires on an import |
| --- | --- | --- |
| `deny` — the package is forbidden outright | **yes** | yes |
| `packages` + `allowed_in` — the package is scoped to modules | **no** | yes |

A denylist matches a declaration because a rule broken in a manifest is still broken and the import
is one commit away. Scoping does not, because scoping is a statement about *where code lives*, and a
manifest declaration has no module location — a finding there could only say the package is "used in
an unassigned path", which says nothing about the architecture.

The practical consequence: **declaring a scoped package in the wrong crate's manifest is not a
violation until something imports it.** It will usually surface as an `unused-dep` warning instead.
If you need the declaration itself forbidden, use `deny` and scope by writing separate rules, or
accept that the import is the thing being governed.

### Denylist

```toml
[[package_rules]]
id = "one-logging-stack"
deny = ["log4j", "logback"]
replacement = "slf4j"      # optional, named in the finding
reason = "..."
```

Fires even if the package is never declared — an import of a transitively-available package is
still a use of it.

### Scoping — a package allowed only in certain modules

```toml
[[package_rules]]
id = "sql-stays-in-the-store"
packages = ["sqlx", "diesel"]
allowed_in = ["store"]
reason = "A SQL helper above the storage layer means queries have escaped it."
```

The most useful package rule shape in practice. `allowed_in` names modules, which must be defined.

### Closed world — an approved list

```toml
[packages]
unlisted = "deny"          # or "allow", the default

[[package_rules]]
id = "approved-dependencies"
allow = ["serde", "tokio", "clap"]
reason = "This service ships to a regulated environment."
```

With `unlisted = "deny"`, any package not on an `allow` list is a violation. This is the shape
regulated teams want and it is unforgiving — expect to add entries. Introduce it as
`severity = "warning"` first.

## Exclusions

```toml
exclude = ["demo/**", "**/testdata/**"]
```

Applied before discovery, so excluded paths are never analyzed. Every exclusion is **disclosed in
each report with a count of what it matched**, and a pattern matching nothing is flagged — an
exclusion is a blind spot, and a silent blind spot defeats the point of the tool.

Use for deliberately-broken sample projects, vendored trees, and generated code. Do not use it to
make a failing gate pass.

## Rejected at parse time

These appear in tropism's design documents but are **not implemented**, and the parser rejects them
by name rather than with a confusing unknown-field error:

| Field | Error |
| --- | --- |
| `layers` | `uses 'layers', which is specified in design/11-dependency-rules.md but not implemented yet` |
| `require` | same |
| `transitive` | same |

Version constraints in package rules are likewise unimplemented. If you want layering today, express
it as a set of `deny` or `allow_only` rules — verbose, but it works and it means exactly what it
says.

This is deliberate: a ruleset must never appear to enforce more than it does.

## Worked examples

### A layered service

```toml
schema_version = 1

[modules]
entry = "cmd/**"
api   = "internal/api/**"
core  = "internal/core/**"
store = "internal/store/**"

[[module_rules]]
id = "entrypoint-composes-the-api"
allow_only = { from = "entry", to = ["api"] }
reason = """
The entrypoint wires up the api and nothing else. Reaching into storage from main
means the process cannot be reused as a library, and every caller inherits a
database connection it never asked for.
"""

[[module_rules]]
id = "core-is-a-leaf"
allow_only = { from = "core", to = [] }
reason = "The domain owns the rules and depends on nothing, so it can be tested without a world."

[[package_rules]]
id = "sql-stays-in-the-store"
packages = ["sqlx"]
allowed_in = ["store"]
reason = "A SQL helper above the storage layer means queries have escaped it."
```

### Two surfaces over one core

```toml
[modules]
cli    = "crates/cli/**"
server = "crates/server/**"
core   = "crates/core/**"

[[module_rules]]
id = "surfaces-are-independent"
independent = ["cli", "server"]
reason = """
Both are adapters over one analysis core. Shared behaviour belongs in core, not in
a dependency between the two surfaces — otherwise the CLI cannot ship without the
server's dependency tree.
"""
```

### Adopting against an existing backlog

```toml
[[module_rules]]
id = "ui-framework-stays-out-of-the-domain"
deny = { from = "domain", to = ["ui"] }
severity = "warning"        # error once the existing violations are cleared
reason = "..."
```

Pair with `tropism check <changed-files>` in a pre-commit hook: the backlog stops growing
immediately, and the severity can be raised to `error` when it reaches zero.
