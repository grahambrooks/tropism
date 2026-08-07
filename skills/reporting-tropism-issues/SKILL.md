---
name: reporting-tropism-issues
description: Turn an experience of using tropism — a false positive, a check that would not run, a missing language or ecosystem, a wish for a feature — into a bug report or feature request that can actually be acted on, after first checking whether the behaviour is a known structural limit rather than a defect. Use this whenever someone is frustrated by a tropism finding, thinks tropism got something wrong, wants tropism to support something it does not, says a check reported "unavailable" and they do not know why, or asks how to report a problem or request a feature. Also use it before filing any issue against tropism, so a known limitation is not re-reported as a bug.
---

# Reporting a tropism issue

Most surprising tropism behaviour is deliberate, documented, and unfixable without giving up the
property that makes the tool useful. A smaller amount is a genuine gap someone would like to hear
about. This skill is about telling those apart *before* filing, and then writing the second kind so
it can be acted on.

Doing the triage first is not gatekeeping. A report that says "this is S3, and here is why it still
hurts me" is far more actionable than one that says "version-conflict doesn't work", because it
starts the conversation past the part the maintainers already know.

## The one distinction that decides everything

tropism **never invokes a package manager and never executes the code it analyzes.** It reads
manifests, lockfiles, and source. That single constraint is the reason it can run in a pre-commit
hook on a fresh checkout with no toolchain — and it is the cause of most of its limitations.

So every surprise sorts into one of two piles:

- **Structural** — a consequence of that constraint. These do not get fixed; they get *reported
  honestly*. Anything that would resolve them trades away the property that makes tropism worth
  having. Filing one as a bug wastes your time and theirs.
- **Deferred** — simply not built yet, with no obstacle beyond effort. These are worth filing, and
  a good report can move them up the list.

The project keeps both in one register. **Check it before filing:**
`design/12-known-limitations.md`, structural entries numbered `S1`–`S8` and deferred ones `D1`–`D36`.

## Triage first

Match the experience against the common cases before writing anything. See
[references/known-limitations.md](references/known-limitations.md) for the full list with the
reasoning behind each.

The ones that account for most reports:

| What you saw | Likely | Worth filing? |
| --- | --- | --- |
| `unused-dep` flagged something that *is* used | **S1**, structural. 63% false positives measured. | Only with a *new* usage channel not already listed. |
| A check said `unavailable` | **S3**, structural for that ecosystem — the lockfile has no edges, or there is none. | No, unless the reason names the wrong file or is inaccurate. |
| `version-conflict` on something your build never compiles | **S8**, structural. A lockfile is feature- and target-agnostic. | Yes, if you have an idea for wording that conveys this better. |
| `diamond-dep` finds nothing in Python/Ruby/Swift | **S7**, correct behaviour. Those install one version per package, so no diamond exists. | No. |
| A dynamic manifest construct was ignored | **S4**, structural. tropism parses `Gemfile`, `build.gradle`, `Package.swift`, `conanfile.py` and never runs them. | Yes, if a *common declarative* form is being missed. |
| An import resolved to the wrong package, or not at all | **S5**, partly structural. Import name ≠ package name. | **Yes** — a missing entry in a curated mapping table is a genuinely useful report. |
| A language is unsupported | Ten are built. An eleventh is effort, not obstacle. | Yes. |
| A rule kind was rejected at parse time | `layers`, `require`, `transitive`, version constraints are **specified but unimplemented** (D6, D7) — rejected by name so a ruleset never appears to enforce more than it does. | Yes, to signal demand. |

If it is structural and you file anyway, **say so in the report**. "I know this is S1, but here is
why the current behaviour is still costing me something" is a legitimate and useful issue. What is
not useful is a report that reads as though the limitation were unknown.

## Gather evidence that makes the report reproducible

tropism is hermetic, which is a gift for bug reports: **the input is just files.** No environment, no
installed dependencies, no network state. A reproduction is usually a handful of small files.

Collect:

```sh
tropism --version
tropism analyze . --format json > report.json     # or the narrower command you ran
```

For anything about a **single import** being classified wrongly, attach the explanation as well — it
turns "tropism thinks this is missing" into the precise step that went wrong, and it is usually
shorter than the report:

```sh
tropism explain path/to/the/file.ext --format json
```

For anything about a **missing-dep that did or did not fire in a monorepo**, attach the boundaries:

```sh
tropism workspaces . --format json
```

An import needs no declaration if another project in the same workspace publishes it, so this is
frequently the answer rather than a bug. Check first whether the workspace holding the two projects
has origin `language` — that is an inference tropism made because the ecosystem declares no
workspace, and correcting it is a `[[workspaces]]` entry rather than an issue.

Then reduce. The single most valuable thing you can attach is a **minimal reproduction**: the
smallest directory that still shows the behaviour. Often that is one manifest and one source file.
Because tropism never installs anything, a fake manifest naming packages that do not exist works
fine — which makes minimising far easier than for most tools.

Check the reduction still reproduces before attaching it. A repro that does not reproduce sends
someone down the wrong path.

## Write the report

Structure it so the first paragraph is enough to triage. Titles that name the check and the surprise
(`missing-dep: import of "yaml" not matched to declared PyYAML`) beat titles that name a feeling
(`hygiene checks are broken`).

```markdown
## What happened
[Command run, and the finding — paste the actual output, not a paraphrase]

## What I expected
[And why. If it is a judgment call, say so.]

## Repro
[Minimal files, or a link. State the tropism version and the ecosystem.]

## Triage
[Which register entry this looks like, if any, and why it still matters —
 or "I could not find this in design/12".]
```

Paste output verbatim. A finding carries a check name, severity, confidence, the file and line, and
the evidence; a paraphrase loses the parts that identify what happened. Include the `checks` section
too, since `unavailable` on a check often explains the whole report.

## Feature requests

A feature request lands better when it says what you were trying to *do*, not only what you want
built. The maintainers have a documented view of what the tool is for — one ruleset enforced at
commit time and repository-wide, across many languages, with no build and no install — and a request
that connects to that will be evaluated on its merits rather than against it.

Useful to include:

- **The situation.** What repository, what languages, what you were trying to prevent.
- **What you do today instead.** A workaround that exists is evidence the need is real.
- **Whether it needs a package manager, a build, or code execution.** If it does, say so — that does
  not make it unwelcome, but it is the first question anyone will ask, and pretending otherwise
  wastes a round trip.

Requests that reliably land well:

- **An entry for a mapping table.** "`import cv2` should resolve to `opencv-python`" is small,
  verifiable, and directly reduces false findings.
- **A new language or ecosystem**, with a description of the manifest and lockfile formats and
  whether the lockfile carries dependency *edges*. That last detail decides which checks can work at
  all.
- **A rule kind you need**, with the constraint you are trying to express in prose. The prose is the
  valuable part — it may be expressible with rules that already exist.
- **Wording that misled you.** A finding whose message caused a wrong action is a real defect even
  when the analysis was right.

Requests that usually do not:

- Anything requiring an installed dependency tree — that is the constraint the tool is built on.
- Making `unused-dep` reliable. It cannot be made reliable offline; that is measured, not assumed.
- Auto-fixing findings. Native tooling already fixes what it detects, and tropism deliberately does
  not compete there.

## Before you file

- [ ] Checked `design/12-known-limitations.md` for a matching `S` or `D` entry
- [ ] Ran the latest release, and said which version
- [ ] Attached verbatim output, including the `checks` section
- [ ] Reduced to a minimal reproduction, and confirmed it still reproduces
- [ ] Said which ecosystem and which manifest and lockfile are involved
- [ ] If structural, said so and explained why it still matters
