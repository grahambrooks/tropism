# Skills for people using tropism

Three skills for coding agents working in a repository that uses tropism. They ship here for the
same reason `.pre-commit-hooks.yaml` does: the thing consuming tropism is often not a human typing
commands, and it needs to be told how to use it well.

| Skill | For |
| --- | --- |
| [`authoring-architecture-rules`](authoring-architecture-rules/) | Writing or repairing a `tropism.toml` from what a repository actually contains |
| [`tropism-in-the-loop`](tropism-in-the-loop/) | Using tropism while writing code, and knowing which findings to act on |
| [`reporting-tropism-issues`](reporting-tropism-issues/) | Turning an experience into a report worth filing, after checking it is not a known structural limit |

## Installing

Copy the directories into wherever your agent loads skills from. For Claude Code:

```sh
# for one project
mkdir -p .claude/skills && cp -R /path/to/tropism/skills/* .claude/skills/

# or for every project you work on
mkdir -p ~/.claude/skills && cp -R /path/to/tropism/skills/* ~/.claude/skills/
```

They need nothing but `tropism` on PATH. Install that from
[releases](https://github.com/grahambrooks/tropism/releases) — one line, no admin rights.

## Why these three

They map onto the three moments a user meets tropism, and each encodes something that is easy to get
wrong and expensive to get wrong:

**Authoring.** The failure is a ruleset that looks fine and enforces nothing — globs that match no
directory, or a rule that cannot fire on its own motivating example. The skill's core instruction is
to prove each rule can fail before trusting it.

**In the loop.** The failure is acting on the wrong finding. Rules and cycles are trustworthy;
`unused-dep` measured 63% false positives and must never drive a deletion; `version-conflict`
describes the lockfile rather than your build. Getting that ordering wrong wastes time in one
direction and breaks builds in the other.

**Reporting.** The failure is re-filing a known structural limit, or filing a real bug as though it
were one. The skill triages against the limitations register first, so what does get filed starts
past the part maintainers already know.

## A note on what these replace

`design/09-product-review.md` once concluded that an MCP server should be the product, on the
grounds that agents needed a way to query tropism and no competitor served them. That
recommendation failed its own gate — see the revision at the end of that document.

These skills serve much of that need at a fraction of the cost. They need no server, no protocol,
and no new surface to maintain; they are three markdown files that make an agent use the CLI well.
Worth remembering if the MCP question is reopened.

## Testing

Exercised against this repository and a neutral fixture, five scenarios each run with and without
the skill. Notes and the eval definitions are in [`evals/evals.json`](evals/evals.json).

The measurement has a known limitation worth stating: baselines run inside this repository can read
`CLAUDE.md` and `design/`, which already document much of what the skills teach, so the measured
delta understates the value for users who have tropism installed but not checked out. The runs did
surface several real defects in tropism itself, which is its own kind of evidence that the workflow
they describe is worth following.
