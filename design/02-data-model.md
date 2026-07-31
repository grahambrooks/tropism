# 02 — Data model

Types below are illustrative Rust, not a committed API. They fix the *shape* of the data and the
relationships between the pieces; field names will move.

## Two graphs, deliberately separate

The most important structural decision in gdep: the internal module graph and the external package
graph are different graphs, built from different sources, answering different questions. Merging
them into one "dependency graph" would produce a structure where no algorithm is correct for all
nodes.

|            | Module graph                     | Package graph                             |
| ---------- | -------------------------------- | ----------------------------------------- |
| Nodes      | Modules/files inside the project | Third-party packages                      |
| Edges      | "imports"                        | "depends on", version-resolved            |
| Built from | Source imports                   | Lockfile (resolved) + manifest (declared) |
| Depth      | Whole project                    | Full transitive tree                      |
| Answers    | Is my own code tangled?          | Is my supply chain a mess?                |

The only place they touch is manifest hygiene, which asks whether an import in the module graph
corresponds to a declared node in the package graph. That comparison is one analyzer's job, not a
property of the graphs.

## Core types

```rust
/// A directory containing a recognized manifest. The unit of analysis.
struct Project {
    root: PathBuf,          // relative to the scan root
    language: Language,
    manifests: Vec<PathBuf>,
    lockfile: Option<PathBuf>,
}

/// A dependency as the manifest declares it: a name and a version *requirement*.
struct DeclaredDep {
    name: PackageName,
    requirement: String,     // raw, uninterpreted: "^1.2", ">=3,<4", "1.0.+"
    kind: DepKind,
    source: Provenance,      // which file and line declared it
}

enum DepKind { Runtime, Dev, Build, Optional, Peer }

/// A dependency as the lockfile resolved it: a name and an exact version.
struct ResolvedDep {
    name: PackageName,
    version: Version,
    dependencies: Vec<PackageId>,   // edges in the package graph
}
```

`DepKind` matters more than it looks. A dev-dependency unused by `src/` is not unused — it is used
by tests. Every analyzer that compares declarations against usage must be `DepKind`-aware, and the
mapping from each ecosystem's own vocabulary (`devDependencies`, `[dev-dependencies]`, `<scope>test`,
`group :development`) onto this enum belongs in the provider.

**Version comparison is per-ecosystem.** SemVer, PEP 440, Maven's ordering, and RubyGems all differ
in ways that matter for conflict detection. Do not assume a single global `Version` type will do;
either keep versions opaque and delegate comparison to the provider, or make `Version` an enum over
per-ecosystem representations. Getting this wrong silently produces wrong conflict findings.

## Findings

```rust
struct Finding {
    id: FindingId,           // stable across runs; see below
    check: CheckId,          // "cycle", "unused-dep", ...
    severity: Severity,      // Error | Warning | Info
    confidence: Confidence,
    project: PathBuf,
    message: String,         // one line, human-readable
    evidence: Vec<Evidence>, // what supports the claim
    details: CheckDetails,   // check-specific payload, e.g. the cycle path
}

struct Evidence {
    file: PathBuf,
    line: Option<u32>,
    note: String,            // "imported here", "declared here"
}

enum Confidence { High, Medium, Low }
```

**Evidence is mandatory, not decorative.** A finding an agent cannot verify is a finding an agent
must take on faith, and this tool will be wrong sometimes. Every finding points at the file and line
that support it.

**Finding IDs must be stable** across runs so a result can be suppressed, referenced, or diffed.
Derive the ID from the content of the finding (check + project + the identifying elements, such as
the sorted cycle members) — never from iteration order or array index.

## Confidence

Confidence describes how much the *analysis method* is trusted for this finding, independent of
severity.

- **High** — derived from declarative data with no inference. A cycle found in explicitly resolved
  imports. A duplicate version read straight from a lockfile.
- **Medium** — sound method, known incomplete inputs. An unused dependency in a language where
  reflection or dynamic import could use it invisibly.
- **Low** — heuristic, or derived from a partially-parsed manifest. Anything read out of
  `build.gradle` that used dynamic constructs.

An analyzer that cannot run at all is not a low-confidence finding. It is a `CheckStatus` of
`Unavailable { reason }` in the report, which is why the report carries per-check status alongside
findings:

```rust
struct Report {
    projects: Vec<ProjectReport>,
    skipped: Vec<SkippedFile>,   // parse failures, so silence is never mistaken for cleanliness
}

struct ProjectReport {
    project: Project,
    checks: BTreeMap<CheckId, CheckStatus>,
    findings: Vec<Finding>,
}

enum CheckStatus {
    Ran { finding_count: usize },
    Unavailable { reason: String },   // "no lockfile; resolved tree unknown"
    Failed { error: String },
}
```

A consumer that sees zero findings must be able to distinguish "clean" from "never ran". That
distinction is the entire purpose of `CheckStatus`, and it is the reason findings alone are not a
sufficient output type.

## Ordering

`Report` fields serialize in a deterministic order: projects sorted by path, findings sorted by
`(check, project, id)`, checks in a `BTreeMap`. No `HashMap` may appear in a serialized type.
