#!/usr/bin/env bash
#
# Re-pins every repository in the corpus to its current default-branch SHA.
#
# The corpus is pinned rather than tracking branches so a second run is a *diff*
# against the first. An unpinned corpus makes every difference ambiguous between
# "tropism changed" and "the repository changed" — which is the one question the
# evaluation exists to answer.
#
# Needs `gh` authenticated. Re-pinning invalidates prior results, by design.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORPUS="$HERE/corpus.tsv"
tmp="$(mktemp)"

{
  printf '# repo\tsha\tlanguages\tshape\tnote\n'
  printf '# Pinned %s. Re-pin with evaluation/pin-corpus.sh.\n' "$(date -u +%Y-%m-%d)"
} >"$tmp"

while IFS=$'\t' read -r repo _sha langs shape note; do
  case "$repo" in \#*|"") continue ;; esac
  branch=$(gh api "repos/$repo" --jq '.default_branch')
  sha=$(gh api "repos/$repo/commits/$branch" --jq '.sha')
  printf '%s\t%s\t%s\t%s\t%s\n' "$repo" "${sha:-UNPINNED}" "$langs" "$shape" "$note" >>"$tmp"
  printf '  %-45s %s\n' "$repo" "${sha:0:12}" >&2
done <"$CORPUS"

mv "$tmp" "$CORPUS"
