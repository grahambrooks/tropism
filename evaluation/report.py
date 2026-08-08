#!/usr/bin/env python3
"""Turns the evaluation's raw output into the report design/19 specifies.

The dimensions it can compute, it computes. The ones needing a human — the
hand audits behind D4, and every check in an ecosystem with no oracle — it
lists as outstanding work with the sample already drawn.

That split is the point. A report that quietly omitted the un-auditable half
would read as a clean bill of health for checks nobody looked at, which is the
failure mode `CheckStatus` exists to prevent, one level up.

    ./report.py                     # markdown to stdout
    ./report.py --json              # machine-readable
    ./report.py --audit-sample 20   # draw the D4 audit sample
"""

from __future__ import annotations

import argparse
import collections
import json
import pathlib
import random
import sys

HERE = pathlib.Path(__file__).resolve().parent
RESULTS = HERE / "results"
ORACLES = HERE / "oracles" / "results"
CORPUS = HERE / "corpus.tsv"

# Checks that gate, versus checks that inform. design/10 is why they are not
# reported together: an aggregate over both hides the split that matters.
INFERRED = ["cycle", "unused-dep", "missing-dep", "version-conflict", "diamond-dep"]


def normalise_language(name: str) -> str:
    """Reconciles the two spellings tropism currently emits for one language.

    Found by this harness on its first real run. `Language` derives
    `serde(rename_all = "kebab-case")`, so the JSON contract says `java-script`,
    `type-script` and `c-sharp`, while `Language::as_str()` — which drives the text
    renderer, `tropism workspaces` and `tropism explain` — says `javascript`,
    `typescript` and `csharp`.

    Normalising here keeps the evaluation honest about a defect that is not the
    evaluation's. Remove this once the contract has one spelling; the report will
    keep working either way.
    """
    return name.replace("-", "")


def load_corpus() -> list[dict]:
    rows = []
    for line in CORPUS.read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        repo, sha, langs, shape, note = (line.split("\t") + [""] * 5)[:5]
        rows.append(
            {
                "repo": repo,
                "sha": sha,
                "languages": [normalise_language(x) for x in langs.split(",")],
                "shape": shape,
                "note": note,
                "slug": repo.replace("/", "__"),
            }
        )
    return rows


def load(slug: str) -> dict | None:
    path = RESULTS / f"{slug}.json"
    if not path.exists():
        return None
    try:
        return json.loads(path.read_text())
    except json.JSONDecodeError as exc:
        return {"error": f"unparseable result: {exc}"}


def summarise(report: dict) -> dict:
    """Per-repository facts, computed straight from the JSON contract."""
    projects = report.get("projects", [])
    findings = [f for p in projects for f in p.get("findings", [])]
    by_check = collections.Counter(f["check"] for f in findings)

    status = collections.Counter()
    unavailable_reasons = []
    for project in projects:
        for check, state in project.get("checks", {}).items():
            status[(check, state.get("status"))] += 1
            if state.get("status") == "unavailable":
                unavailable_reasons.append((check, state.get("reason", "")))

    exemptions = [e for p in projects for e in p.get("sibling_exemptions", [])]

    return {
        "projects": len(projects),
        "languages": sorted({normalise_language(p["language"]) for p in projects}),
        "source_files": sum(p.get("source_file_count", 0) for p in projects),
        "findings": len(findings),
        "by_check": dict(by_check),
        "skipped_files": len(report.get("skipped", [])),
        "seconds": report.get("evaluation", {}).get("seconds"),
        "exit_code": report.get("evaluation", {}).get("exit_code"),
        "ran": sum(v for (_, s), v in status.items() if s == "ran"),
        "unavailable": sum(v for (_, s), v in status.items() if s == "unavailable"),
        "failed": sum(v for (_, s), v in status.items() if s == "failed"),
        "unavailable_reasons": unavailable_reasons,
        "sibling_exemptions": len(exemptions),
        "exempted_imports": sum(e.get("imports", 0) for e in exemptions),
    }


def oracle_status(slug: str) -> dict[str, bool]:
    found = {}
    for path in ORACLES.glob(f"{slug}.*.json"):
        try:
            data = json.loads(path.read_text())
        except json.JSONDecodeError:
            continue
        found[data.get("oracle", path.stem)] = bool(data.get("ok"))
    return found


def draw_audit_sample(corpus, size: int, seed: int = 20260808) -> list[dict]:
    """D4's sample: hygiene findings, stratified by language.

    Seeded, so the sample is reproducible and a second auditor grades the same
    findings — otherwise two audits are not comparable.
    """
    rng = random.Random(seed)
    pool: dict[str, list] = collections.defaultdict(list)
    for row in corpus:
        report = load(row["slug"])
        if not report:
            continue
        for project in report.get("projects", []):
            for finding in project.get("findings", []):
                if finding["check"] in ("unused-dep", "missing-dep"):
                    pool[normalise_language(project["language"])].append(
                        {
                            "repo": row["repo"],
                            "language": normalise_language(project["language"]),
                            "check": finding["check"],
                            "id": finding["id"],
                            "message": finding["message"],
                            "evidence": finding.get("evidence", [])[:2],
                        }
                    )
    sample = []
    for language, findings in sorted(pool.items()):
        rng.shuffle(findings)
        sample.extend(findings[:size])
    return sample


def markdown(corpus) -> str:
    out: list[str] = []
    w = out.append

    loaded = [(row, load(row["slug"])) for row in corpus]
    done = [(r, d) for r, d in loaded if d and "error" not in d]
    missing = [r for r, d in loaded if d is None]
    errored = [(r, d) for r, d in loaded if d and "error" in d]

    w("# Analysis evaluation — results\n")
    w(f"Corpus: **{len(done)}/{len(corpus)} analyzed**"
      + (f", {len(missing)} not yet run" if missing else "")
      + (f", **{len(errored)} failed**" if errored else "")
      + ".\n")
    w("Generated by `evaluation/report.py`. Dimensions follow "
      "[design/19-analysis-evaluation.md](../design/19-analysis-evaluation.md).\n")

    if errored:
        w("## D7 — Robustness: failures\n")
        w("Any non-finding failure is a P1 regardless of every other number.\n")
        for row, data in errored:
            w(f"- **{row['repo']}** — {data.get('error')}")
        w("")

    # ---------------------------------------------------------------- D1
    w("## D1 — Discovery\n")
    w("Gates everything below: a missed project silently removes code from every "
      "other number.\n")
    w("| Repository | Shape | Projects | Languages found | Expected | Source files |")
    w("| --- | --- | --- | --- | --- | --- |")
    for row, data in done:
        s = summarise(data)
        expected = ", ".join(sorted(row["languages"]))
        found = ", ".join(s["languages"]) or "**none**"
        flag = "" if set(s["languages"]) >= set(row["languages"]) else " ⚠"
        w(f"| {row['repo']} | {row['shape']} | {s['projects']} | {found}{flag} "
          f"| {expected} | {s['source_files']:,} |")
    w("\n⚠ marks a language the corpus expects and discovery did not find. For C++ "
      "this is expected on most repositories and is a property of the ecosystem, "
      "not a defect — see design/19.\n")

    # ---------------------------------------------------------------- D5/D3
    w("## D3/D5 — Findings by check\n")
    w("Counts only. Soundness needs the oracle columns and the hand audit below; "
      "a count is not a verdict.\n")
    w("| Repository | " + " | ".join(INFERRED) + " | Oracles available |")
    w("| --- | " + " | ".join("---" for _ in INFERRED) + " | --- |")
    for row, data in done:
        s = summarise(data)
        cells = [str(s["by_check"].get(check, 0)) for check in INFERRED]
        oracles = oracle_status(row["slug"])
        got = ", ".join(k for k, ok in sorted(oracles.items()) if ok) or "—"
        w(f"| {row['repo']} | " + " | ".join(cells) + f" | {got} |")
    w("")

    # ---------------------------------------------------------------- D2
    w("## D2 — Check availability\n")
    w("`unavailable` is not a failure — it is the honest answer where no resolved "
      "tree exists (S3). What matters is whether the *reason* is right.\n")
    reasons = collections.Counter()
    for _, data in done:
        for check, reason in summarise(data)["unavailable_reasons"]:
            reasons[(check, reason[:90])] += 1
    w("| Check | Reason | Projects |")
    w("| --- | --- | --- |")
    for (check, reason), count in reasons.most_common(15):
        w(f"| {check} | {reason}… | {count} |")
    w("")

    # ---------------------------------------------------------------- D6
    w("## D6 — Workspace boundaries and sibling exemptions\n")
    w("An exemption is a deliberate blind spot. This is the first time its size "
      "has been seen on real code.\n")
    w("| Repository | Packages exempted | Imports covered |")
    w("| --- | --- | --- |")
    for row, data in done:
        s = summarise(data)
        if s["sibling_exemptions"]:
            w(f"| {row['repo']} | {s['sibling_exemptions']} | {s['exempted_imports']} |")
    w("\nRun `tropism workspaces <repo>` to check origins; any `language` origin "
      "where the ecosystem *does* declare a workspace is a bug in the D14 work.\n")

    # ---------------------------------------------------------------- D7/D8
    w("## D7/D8 — Robustness and scale\n")
    w("| Repository | Seconds | Source files | Files/sec | Skipped files |")
    w("| --- | --- | --- | --- | --- |")
    for row, data in done:
        s = summarise(data)
        secs = s["seconds"]
        rate = f"{s['source_files'] / secs:,.0f}" if secs else "—"
        flag = " ⚠" if s["skipped_files"] else ""
        w(f"| {row['repo']} | {secs if secs is not None else '—'} "
          f"| {s['source_files']:,} | {rate} | {s['skipped_files']}{flag} |")
    w("")

    # ---------------------------------------------------------------- D4
    w("## D4 — Manifest hygiene: outstanding hand audit\n")
    w("**Not computable.** design/10 measured 63% false positives on JavaScript by "
      "reading 35 findings against the source; nothing automated can replace that, "
      "and the question here is whether that number is a JavaScript number or a "
      "tropism number.\n")
    pool = collections.Counter()
    for row, data in done:
        for project in data.get("projects", []):
            for finding in project.get("findings", []):
                if finding["check"] in ("unused-dep", "missing-dep"):
                    pool[normalise_language(project["language"])] += 1
    if pool:
        w("| Language | Hygiene findings available to sample |")
        w("| --- | --- |")
        for language, count in sorted(pool.items()):
            w(f"| {language} | {count} |")
        w("\nDraw the sample with `./report.py --audit-sample 20` — seeded, so a "
          "second auditor grades the same findings and the two audits are "
          "comparable.\n")
    else:
        w("No hygiene findings in the corpus yet.\n")

    w("## Not covered by any oracle\n")
    w("C#, C++ and Swift have no automated ground truth (see "
      "`oracles/Dockerfile.oracles`). Their numbers above are **unverified counts**, "
      "not measured accuracy, and must not be reported as though they were.\n")
    return "\n".join(out)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="machine-readable summary")
    parser.add_argument("--audit-sample", type=int, metavar="N",
                        help="draw N hygiene findings per language for the D4 audit")
    args = parser.parse_args()

    corpus = load_corpus()
    if not RESULTS.exists() or not any(RESULTS.glob("*.json")):
        print("no results yet — run ./run.sh first", file=sys.stderr)
        return 1

    if args.audit_sample:
        print(json.dumps(draw_audit_sample(corpus, args.audit_sample), indent=2))
        return 0
    if args.json:
        payload = {
            row["repo"]: summarise(data)
            for row in corpus
            if (data := load(row["slug"])) and "error" not in data
        }
        print(json.dumps(payload, indent=2, default=str))
        return 0

    print(markdown(corpus))
    return 0


if __name__ == "__main__":
    sys.exit(main())
