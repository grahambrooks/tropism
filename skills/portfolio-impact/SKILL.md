---
name: portfolio-impact
description: Work out what a change to one of your own libraries affects across your other repositories — which repos use it, how they are pinned, which call sites would break, and in what order to release. Use this when about to change, rename or remove a public API of a library that other repositories of the same owner depend on (as a Cargo git dependency, a Go module, an npm package, or an installed CLI), when asked "who uses X?", "what breaks if I change X?", "which repos are behind X's newest tag?", "does X need a release?", or "in what order do I release these?", and before cutting a tag of a library that has users. Requires the tropism CLI; uses symgraph or colab for call sites when available.
---

# Portfolio impact

One repository cannot tell you who depends on it. `tropism portfolio` can: it reads every checkout
under a set of roots and reports the links between the owner's repositories, with each link's pin,
what the lockfile resolved, how many tags behind the user is, and whether the library has commits
waiting on a release. This skill turns that graph into an impact answer — **users, call sites,
release order** — without building anything.

The graph answers *which repositories*. It does not answer *which lines*; that is a second step
per user, and getting it wrong is the common failure (see "Call sites", below).

## 1. Get the graph

```sh
tropism portfolio ~/dev/projects ~/dev/sustain --owner <owner> --json > /tmp/portfolio.json
```

- Roots are directories holding checkouts, or checkouts themselves. Include every place a user
  might be cloned: a repository that is not checked out contributes nothing, and its absence is
  **not** evidence that it does not depend on the library.
- `owner` can come from `tropism-portfolio.toml` in the first root instead of `--owner`.
- Tags come from each checkout's refs as last fetched (`"tags_from": "local"`). If the question is
  about *right now* — "is there an unreleased change?" — add `--fetch` (it lists remote tags with
  `git ls-remote`; it never runs `git fetch`), or say the answer is as of the last fetch.
- Exit `2` means it did not run (no owner, a rules file that does not load). Never read an empty
  result from a failed run as "no users".

## 2. Find the users

```sh
LIB=diagram-scene
jq --arg lib "$LIB" '.edges[] | select(.to == $lib)
  | {from, kind, package, via, through, pinned: .pinned.value, behind_by, behind_note, confidence}' \
  /tmp/portfolio.json
jq --arg lib "$LIB" '.libraries[] | select(.repo == $lib) | {newest_tag, release, users}' /tmp/portfolio.json
jq -r --arg u ferrisite '.repos[] | select(.name == $u) | .path' /tmp/portfolio.json   # where a user lives
```

Sort the users before looking at code, because they are affected differently:

| Edge | What a change does to it |
| --- | --- |
| `via: direct`, `kind: cargo/go/npm` | Calls the API. Breaks when it next bumps to a tag containing the change. |
| `via: transitive` (`through: [mmd]`) | Reaches the library only through `through`. Affected when *that* library adopts the change; check whether it re-exports the changed API. |
| `pinned.kind: path` | Builds against the library's working tree. Breaks **immediately**, before any release. |
| `pinned.kind: branch` / `default-branch` | Breaks on its next `cargo update`. |
| `kind: binary` | Runs the CLI. Affected only by a change to its command line or output. Medium confidence: it came from text matching. |
| `behind_by > 0` | Already that many releases behind. Its call sites compile against the **pinned** tag, not the library's HEAD, so the change lands on top of drift it already owes. |
| `behind_by` absent | Unknown, and `behind_note` says why. Never treat it as zero. |

A library whose `status` is `unavailable` was linked to but is not checked out: say its impact is
unknown rather than leaving it out.

## 3. Find the call sites in each user

**symgraph's `references` and `callers` will not find them.** They resolve a symbol from its
definition *inside the queried root*, and the library's function is defined in the library, not in
the user — so `symgraph-references to_svg root:<user>` reports "not found" even where the user calls
it. Use symgraph in the *library* (to see what the changed symbol touches and what wraps it), and a
structural search in the *users*.

In each user, search for the call shape as the user's source writes it — usually fully qualified,
and sometimes imported and called bare. With colab:

```
refactor "find to_svg" {
  match rust::pattern 'diagram_scene::svg::to_svg($A, $B)' { delete }
  match rust::pattern 'to_svg($A, $B)' { delete }
}
```

Run it with `colab_find` (or `colab find`) over each user's source directory, with `cwd` set; nothing
is changed by a find. A pattern matches the path exactly as written: `svg::to_svg(…)` does not
match `diagram_scene::svg::to_svg(…)`, so list every spelling the users might use, including the
`use`-imported bare call. Cross-check with a plain `grep -rn <name>` over the same directories; when
the two disagree, read the lines. The crate name in source is the package name with `-` → `_`
(`diagram-scene` → `diagram_scene`), and `package` in the edge is the name to search for, which can
differ from the repository (`forge` publishes `forge-dsl`).

For an MCP-connected symgraph whose server is rooted somewhere else, pass `root` as the **absolute
path** from `repos[].path`; a bare repository name resolves only when the server was started over
the workspace that contains it.

**Then follow the intermediates.** A transitive user is affected only if the library in `through`
exposes the changed behaviour. Find the intermediate's own call sites of the changed API with the
same structural search, note the function each sits in, and ask symgraph who calls *that* function
(`symgraph-callers render_with root:<intermediate path>` works, because the wrapper is defined
there). Then search the users for the wrapper's public entry points. On the real portfolio, mmd wraps `diagram_scene::svg::to_svg`
in `render_with`, but emx, ferrisite and sightline call `mmd::scene` and render it themselves, so
mmd's wrapper reached none of them.

## 4. Release order

Libraries before their users, intermediates in between: a library that is also a user (`from` in
some edges and `to` in others) releases after everything it depends on and before everything that
depends on it.

```sh
jq -r '.edges[] | select(.kind != "binary") | "\(.to) \(.from)"' /tmp/portfolio.json | sort -u | tsort
```

`tsort` prints a valid order and reports a cycle, which would be a real problem worth raising.

Watch for **two copies**. If a user bumps a library directly while an intermediate still pins the
old tag, the user's lockfile resolves both. `tropism portfolio` reports it as `duplicate-copy` —
"the lockfile resolves N copies", which is a fact about the lockfile, not proof that both are
compiled. Releasing the intermediate first avoids it.

A library with `release.status: pending` has commits its users would get that no tag has released
(lock-only and release-bot commits excluded). Users cannot pick up the change until that tag
exists. Cutting it is a release decision for the person who owns the library, so recommend it
rather than doing it.

## 5. Report

One table, then the order:

| User | Link | Pinned → newest | Call sites | Effect |
| --- | --- | --- | --- | --- |
| ferrisite | cargo, direct | v2026.10.25 → v2026.11.12 (3 behind) | `src/diagrams.rs:128`, `:132` | breaks on bump |
| emx | cargo, direct | v2026.10.25 → v2026.11.12 (3 behind) | none | rebuilds only |
| backstage-rs | cargo, via puml | v2026.10.25 | none, through puml | none unless puml exposes it |

State what was not checked: unavailable libraries, users not checked out, binary links, and
`tags_from: local` when the answer depends on what is on the remote.

## Don't

- **Don't build or update the users to find out.** That is a downstream check (sustain does it with
  `[patch]` in an export of each user); this skill is the read-only half that says who to check.
- **Don't treat binary links as call sites.** A Makefile running a tool says nothing about its API.
- **Don't drop the absent cases**: an unavailable library, a missing checkout, a `behind_note`.
