#!/usr/bin/env bash
#
# Dismiss Dependabot alerts raised against the deliberately-broken sample projects
# in demo/.
#
# Why this script exists rather than a config file: **Dependabot alerts cannot be
# filtered by path.** `.github/dependabot.yml` controls update *pull requests*;
# alerts come from the dependency graph, which discovers every manifest by
# filename. Auto-triage rules can match severity, package, and CWE — not
# directory. The only directories the graph skips are `third-party/`, `vendor(s)/`
# and `extern*/`, and renaming `demo/` to one of those would be both untrue and a
# break: scripts/demo.sh, tropism.toml, the READMEs, and demos.rs all name it.
#
# So the alerts are dismissed, with the reason GitHub provides for exactly this
# situation: `not_used`. It is accurate rather than convenient. Nothing under
# demo/ is ever installed, built, or shipped — tropism reads those manifests as
# *text*, which is the entire point of the tool. The packages named in them do not
# exist on any machine.
#
# Run after adding a demo, or whenever the noise comes back:
#
#     ./scripts/dismiss-demo-alerts.sh          # show what would be dismissed
#     ./scripts/dismiss-demo-alerts.sh --apply  # dismiss them
#
# Anything outside demo/ is never touched, and the script says how many such
# alerts exist. That number is the one worth reading: it is the repository's real
# vulnerability count, which the demo noise otherwise buries.

set -euo pipefail

REPO="${TROPISM_REPO:-grahambrooks/tropism}"
APPLY=false
[[ "${1:-}" == "--apply" ]] && APPLY=true

command -v gh >/dev/null || { echo "needs the gh CLI" >&2; exit 2; }

alerts=$(gh api "repos/${REPO}/dependabot/alerts?state=open&per_page=100" --paginate)

demo_count=$(jq -r '[.[] | select(.dependency.manifest_path | startswith("demo/"))] | length' <<<"$alerts")
real_count=$(jq -r '[.[] | select(.dependency.manifest_path | startswith("demo/") | not)] | length' <<<"$alerts")

echo "open alerts: $((demo_count + real_count))"
echo "  demo/ fixtures : ${demo_count}  (never installed, never shipped)"
echo "  everything else: ${real_count}  <- this is the number that matters"
echo

if [[ "${real_count}" -gt 0 ]]; then
  echo "alerts outside demo/, which this script will NOT touch:"
  jq -r '.[] | select(.dependency.manifest_path | startswith("demo/") | not)
         | "  [\(.security_advisory.severity)] \(.dependency.package.name) in \(.dependency.manifest_path)"' <<<"$alerts"
  echo
fi

if [[ "${demo_count}" -eq 0 ]]; then
  echo "nothing to dismiss."
  exit 0
fi

if [[ "${APPLY}" != true ]]; then
  echo "would dismiss ${demo_count} alert(s) as not_used:"
  jq -r '.[] | select(.dependency.manifest_path | startswith("demo/"))
         | "  #\(.number) [\(.security_advisory.severity)] \(.dependency.package.name) in \(.dependency.manifest_path)"' <<<"$alerts"
  echo
  echo "re-run with --apply to dismiss them."
  exit 0
fi

for number in $(jq -r '.[] | select(.dependency.manifest_path | startswith("demo/")) | .number' <<<"$alerts"); do
  gh api --method PATCH "repos/${REPO}/dependabot/alerts/${number}" \
    -f state=dismissed \
    -f dismissed_reason=not_used \
    -f dismissed_comment='Fixture in demo/. These sample projects are deliberately broken and are never installed, built, or shipped; tropism reads their manifests as text. See scripts/dismiss-demo-alerts.sh.' \
    --jq '"dismissed #\(.number)  \(.dependency.package.name)"'
done
