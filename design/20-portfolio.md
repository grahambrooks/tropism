# 20 — Portfolio: the links between your own repositories

Issue #74. Status: **built** — `tropism portfolio`, in the `tropism-portfolio` crate.

## The problem

A team's repositories depend on each other. ferrisite and emx use mmd, puml, forge-dsl and
diagram-scene as Cargo git dependencies pinned to tags; gb-agent-skills pins tropism, symgraph and
colab releases in `tools.toml`. Before this, one tool knew about those links, and only while it ran:
sustain rebuilt the graph on every run (`src/cargo/git_deps.rs`), for Rust git-tag dependencies
only. You could not ask "who uses mmd?", "which users are behind its newest tag?", or "does any
lockfile hold two copies of diagram-scene?" — the last was a note kept by hand in the workspace
runbook.

## What it is, and what it is not

`tropism analyze` and `tropism check` answer questions about one repository. `tropism portfolio`
answers the ones that span several, and outputs the **links between repositories** as a versioned
JSON document:

```sh
tropism portfolio ~/dev/projects ~/dev/sustain --owner grahambrooks --json
```

It extends the thesis — one ruleset, no build, no install — from one repository to many. It does
**not** add a "finds dependency problems" claim; Renovate, Dependabot and `cargo outdated` already
cover third-party freshness, and third-party links are ignored here entirely.

**The single-repository analysis is unchanged.** `analyze`, `check`, `workspaces`, `explain`, the
ruleset in `tropism.toml`, the report contract in `tropism-core::report` and the MCP server do not
know the portfolio exists. The new code is a crate beside core, and the CLI is its only adapter.

## Decisions on the issue's open questions

1. **A subcommand, in a separate crate.** `tropism portfolio`, not a second binary: a second binary
   is a second thing to install, release, and keep in step. `tropism check` stays a single-repo tool
   because it is a different subcommand, not because of a binary boundary. The analysis lives in
   `crates/tropism-portfolio`, not in `tropism-core`, because core is language-agnostic and knows
   nothing of version control, and neither is true of anything here. `tropism.toml` enforces it:
   `portfolio-builds-on-core-only`.
2. **A separate `tropism-portfolio.toml`**, found in the first root, or given with `--rules`. The
   directory it lives in — `~/dev/projects` — is usually not a repository, and its keys mean nothing
   inside one. Putting them in `tropism.toml` would make every repository's ruleset carry keys it
   cannot use.
3. **Local tags by default; `--fetch` opts into `git ls-remote`.** Hermetic and fast by default (the
   whole 83-repository workspace runs in 0.6 s), with the staleness stated in every report's
   `limitations` and `tags_from: "local"`. `--fetch` lists remote tags only; it never runs
   `git fetch`, so nothing in a checkout changes.
4. **deps-plan should read this output** rather than rediscover cross-repository links. That is a
   change for deps-plan's own spec, recorded here so it is not rebuilt.

## The core constraint, restated for git

No package manager runs, nothing is built, no lockfile is regenerated. `git` *is* invoked — for
each checkout's remote, branch and HEAD, and for libraries' tags and commit ranges — and that is a
deliberate, bounded exception of the same kind `check --staged` already makes: git is history, not
resolution, it reads no manifest, and it executes nothing from the analyzed repository. Every call
is read-only (`remote get-url`, `for-each-ref`, `log`, `tag --no-merged`, `rev-parse`, and with
`--fetch`, `ls-remote`). The `History` trait keeps it out of the analysis, so tests can substitute
a fixed history.

## Pipeline

1. **Discover.** Each root is a repository itself (it has `.git`) or a directory whose direct
   children are. Nested checkouts are skipped by the walk: each is its own repository. A root with
   no repositories under it is a `note`, not a clean result.
2. **Identify** each checkout by its remote. `git@github.com:o/n.git`, `https://github.com/o/n`,
   `ssh://git@github.com/o/n`, `git+https://…#v1` and `github:o/n` are one repository
   (`remote::parse`). Remotes under `--owner` are **own** repositories, and only own repositories can
   be libraries. Any checkout can be a user. Two checkouts of one remote: the first wins and the
   others are listed in `duplicate_checkouts`.
3. **Scan** each repository's files, honouring `.gitignore`, the repository's own `tropism.toml`
   `exclude` (so `tropism/demo/` stays out exactly as it does for `analyze`), and the portfolio
   file's `exclude` (globs over `<repo>/<path>`). Every exclusion is reported with a match count.
4. **Link.** See the table below.
5. **Facts** from each library's history, read once per library and in parallel.
6. **Findings**, at the severity a portfolio rule sets, or `info` when none does.

### Link discovery

| Kind | What is read | Confidence |
| --- | --- | --- |
| `cargo` | `Cargo.toml` `git` + `tag`/`rev`/`branch` (and `package` renames, `[workspace.dependencies]`, `[target.*]`), `path` into another checkout; `Cargo.lock` `source = "git+URL?tag=…#sha"` | high |
| `go` | `go.mod` `require` (with `// indirect` → `via: indirect`), `replace` (a local replace pins `path`); `go.sum` confirms the resolution | high |
| `npm` | `package.json` git specifiers (`github:o/n#v1`, `git+https://…`, `o/n#ref`, `#semver:`), and registry names an own repository publishes; `package-lock.json` | high for a git specifier, medium for a name match |
| `binary` | tool manifests (`repo` + `ref`, as `tools.toml`, `tools.lock.json`, `prek.toml` spell it), `.pre-commit-config.yaml` `repo`/`rev`, `owner/repo@ref`, release download URLs, `install` lines naming the owner's URL, `brew "owner/tap/formula"`, and command-position uses of an own binary in Makefile recipes, workflows, scripts and Dockerfile `RUN` lines | medium: text matching |

A lockfile entry from an own repository that no declaration accounts for is a **transitive** edge,
with `through` naming the lockfile packages that pull it in. A declaration claims only the copy its
pin produces (`tag = "v1"` claims `?tag=v1`), so a *second* copy of a directly-declared library —
the diagram-scene problem — is still an edge, through whatever reaches it.

**What a repository builds**, for binary links: release-kit's `.release.env` `BINS=` when present
(it is the repository's own statement of what it ships, and it *replaces* inference); otherwise
Cargo `[[bin]]`/`src/main.rs`/`src/bin/*`, Go `main.go`/`cmd/*`, npm `bin`. Manifests under
`examples/`, `tests/`, `fixtures/`, `demo/` and similar ship nothing. Both rules came from the first
real run: colab's example binaries `m` and `demo`, and insight's internal `baseline`, were attributed
as uses wherever a script ran a command by those names. A binary two repositories both build is not
attributed, and says so in `notes`.

**A mention is not a link.** A README link or a comment naming a repository is ignored; a URL counts
only with a pin (`@ref`, a release download) or on an `install` line. A word in command position
counts only if it is not a flag: `-m "…"` on a continuation line and `args+=(--baseline …)` were
both once read as binaries.

### Per-link facts

- **`pinned`** — what the manifest asks for: `tag`, `rev`, `branch`, `default-branch`, `version`
  (Go), `range` (npm), `path`.
- **`resolved`** — what each lockfile recorded, one entry per copy: version, reference, commit.
- **`newest`** — the library's newest **stable version tag**: optional `v`, then exactly three
  numeric components (`v2026.11.12`, `v1.4.0`, and `2026.9.9` — sustain's `tag_version` accepts the
  unprefixed form too, and the two tools must agree).
- **`behind_by`** — stable tags newer than what the user is on. Tag and version pins compare
  numerically; `rev`, a Go pseudo-version, and branch pins with a locked commit count tags not
  merged into that commit (`git tag --no-merged`). When it cannot be established — a path
  dependency, an unavailable library, a commit missing from the library's checkout — it is
  **absent, with `behind_note` saying why**, never a silent zero.
- **Release pending** (per library) — the default branch (`origin/HEAD`, else `origin/main` or
  `origin/master`) has commits since the newest tag that users would get. The test is sustain's
  `release::unreleased`: commits that touch only lockfiles, and release-kit's own `Release v…` bot
  commit, do not count. A merge lists no files and does not count; the commits it merges do.

### Findings

| Check | Default | With a rule | Confidence |
| --- | --- | --- | --- |
| `release-pending` | info | `release_pending = true` → its severity (default warning) | high |
| `tags-behind` | info, for every edge with `behind_by > 0` | `max_tags_behind = N` → its severity (default error) when `behind_by > N` | the edge's |
| `duplicate-copy` | info | `single_copy = [...]` → its severity (default warning) | **medium** (S8) |
| `stale-rule` | — | a rule naming a library or repository that matches nothing → warning | high |

Without a rules file, nothing exceeds `info`, so `tropism portfolio` never fails a build by
accident. `--fail-on` works as everywhere else; exit codes are `0`/`1`/`2`.

## Rules: `tropism-portfolio.toml`

```toml
schema_version = 1
owner = "grahambrooks"              # default for --owner
exclude = ["sustain/evals/**"]      # <repo>/<path> globs; reported with match counts

[[rules]]
id = "diagram-tools-current"
max_tags_behind = 2
libraries = ["mmd", "puml", "diagram-scene"]   # repo or package names; empty = all
users = ["ferrisite", "emx"]                   # empty = all
kinds = ["cargo"]                   # default cargo, go, npm; "binary" must be named
severity = "error"                  # default error: the team asserted it
reason = "the sites render with the released diagram tools"

[[rules]]
id = "one-diagram-scene"
single_copy = ["diagram-scene"]
severity = "warning"                # default warning, until S8's cost here is measured

[[rules]]
id = "release-promptly"
release_pending = true
libraries = ["diagram-scene"]
```

Snake case, like `tropism.toml` (the issue spelled them `max-tags-behind`). As there, an unknown
key, a rule with no kind or two, `release_pending = false` and an unknown `kinds` entry are load
errors (exit 2), so a file never appears to enforce more than it does.

**Binary links never gate unless a rule names `kinds = ["binary"]`.** Bumping a pinned CLI is a
release decision, not a build that breaks; there is no cascade for it.

## Output contract

`schema: "tropism-portfolio"`, `schema_version: 1`. Separate from `tropism analyze`'s contract,
because its unit is a repository rather than a project, but findings keep the same shape and reuse
core's `Severity`, `Confidence`, `Evidence` and `CheckStatus`, so a reader of one already reads the
other. `SCHEMA_VERSION` increments on any breaking change; consumers ignore fields they do not know.

| Field | Content |
| --- | --- |
| `repos[]` | name (remote name; `owner/name` for others; directory name with no remote), path, remote, own, branch, head, packages published, binaries shipped, duplicate checkouts |
| `libraries[]` | repo, remote, `available`/`unavailable`, packages, `newest_tag`, `release` (`current`/`pending`/`no-tag`/`not-comparable`/`unavailable`, branch, commits, reason), users |
| `edges[]` | from, to, kind, package, via (`direct`/`transitive`/`indirect`), through, pinned, resolved[], newest, behind_by / behind_note, confidence, evidence[] |
| `checks` | per check: `ran` with a count, or `unavailable` with a reason (`stale-rule` without a rules file) |
| `findings[]` | id (`check:repo:hash`, stable), check, severity, confidence, repo, library, rule, message, evidence, details |
| `exclusions[]` | pattern, source, matched — including portfolio patterns that matched nothing |
| `notes[]` | files that did not parse, tags that could not be read, ambiguous binaries, empty roots |
| `limitations[]` | the structural limits below, stated in every report |

Output is deterministic: repositories, libraries, edges and findings are sorted, and the same input
produces byte-identical JSON however many threads scanned it (`the_contract_is_versioned_and_stable`).

## Limitations, stated in the output

- **S8 applies to `duplicate-copy`.** A lockfile records neither features nor targets, so two
  copies in `Cargo.lock` do not prove both are compiled. The message says "the lockfile resolves two
  copies", never "the build links two copies", and the confidence is medium.
- **Binary links are text matching.** Medium confidence; they gate only when a rule asks.
- **A repository that is not checked out contributes nothing.** It is listed as an `unavailable`
  library, never as one with no users or dependencies, and its own links are unknown.
- **Local tags are as last fetched.** Without `--fetch`, a newer tag is invisible, and release
  pending compares against the default branch as last fetched.
- **npm resolution reads `package-lock.json` only**; `yarn.lock` and `pnpm-lock.yaml` are not read
  for portfolio links.
- **Go has no duplicate-copy check.** Minimal version selection picks one version per module path;
  two major versions are two module paths, not two copies.

## Measured

On 2026-11-23, against `~/dev/projects` and `~/dev/sustain` (83 checkouts, 72 own): **0.56 s**
wall-clock in a debug build, 20 libraries, 47 links (20 Cargo, 27 binary), no notes beyond one
genuinely ambiguous binary name (`gitatlas`, built by both gitatlas and gitatlas-cli).

- **Cargo links agree with the issue's survey exactly**: diagram-scene 6 direct users (emx,
  ferrisite, forge, mmd, puml, sightline) plus backstage-rs transitively through puml; puml 4, mmd 3,
  adoc 3, forge 2, symgraph 1.
- **ferrisite and its libraries**, checked by hand against git: ferrisite is behind on mmd (4 tags),
  puml (5), diagram-scene (3) and forge-dsl (2), and current on adoc. Every `behind_by` matched a
  hand count of stable tags; every release-pending count matched `git log <tag>..origin/main` after
  removing lock-only and release-bot commits (mmd 2, puml 2, diagram-scene 1, adoc 2).
- **No Go edges**, correctly: the six `go.mod` files the survey found *declare* grahambrooks module
  paths, and none requires another.
- **No duplicate copies today.** emx and ferrisite lock diagram-scene v2026.10.25 both directly and
  through mmd, which is one copy. The finding fires the moment one is bumped without the other, which
  is what the runbook note was tracking by hand.

## Consumers

- **The `portfolio-impact` skill** (`skills/portfolio-impact/`): "what breaks if I change
  `mmd::…`?" — runs `tropism portfolio` to find the users, then symgraph `callers`/`references`
  with `root: <user>` in each. It needs no MCP tool, so the reason MCP was demoted (queries that need
  a resolved tree) does not arise.
- **ggh**: a graph view and attention rules, reading this JSON and changing nothing. Its own issue.
- **sustain**: reads the graph instead of `git_deps.rs`, keeping its downstream check, release
  logic and libraries-first order; Go downstream checks with a temporary `replace`. Its own issue.
- **forge**, optionally: a `portfolio.forge` for a catalog site.

## Not built

- yarn.lock / pnpm-lock.yaml resolution for npm edges (the parsers exist in `tropism-lang` for a
  different question; sharing them means moving them below both crates).
- Symbol-level impact inside tropism itself — that is symgraph's job, composed by the skill.
- An MCP tool for the portfolio. The CLI's JSON is the interface; the skill shows it is enough.
