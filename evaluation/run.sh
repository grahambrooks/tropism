#!/usr/bin/env bash
#
# The tropism pass: clone the pinned corpus and analyze each repository inside the
# hermetic container.
#
# **One repository on disk at a time.** Each checkout is deleted as soon as it has
# been analyzed, because the corpus includes kubernetes, vscode, elasticsearch and
# dotnet/runtime and keeping them all would cost tens of gigabytes to hold data that
# is fully reproducible from `corpus.tsv`. The result JSON is the artefact; the
# clone is scaffolding. `KEEP=1` retains them for debugging.
#
# Resumable and failure-tolerant: a repository that has a result is skipped, and one
# that cannot be cloned or analyzed is *recorded as failed* and the run continues.
# A three-hour run must not die on repository nineteen.
#
#   ./run.sh                 # whole corpus
#   ./run.sh vercel/next.js  # one repository
#   FORCE=1 ./run.sh         # re-analyze even where a result exists
#   KEEP=1 ./run.sh          # keep checkouts (needs far more disk)
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
# shellcheck source=lib.sh
source "$HERE/lib.sh"

CORPUS="$HERE/corpus.tsv"
CHECKOUTS="${CHECKOUTS:-$HERE/.checkouts}"
RESULTS="$HERE/results"
IMAGE="${IMAGE:-tropism-eval}"
MIN_FREE_MIB="${MIN_FREE_MIB:-8192}"

mkdir -p "$CHECKOUTS" "$RESULTS"

record_failure() {
  local out="$1" stage="$2" detail="$3"
  jq -n --arg stage "$stage" --arg detail "$detail" \
    '{error: ("failed at " + $stage), detail: $detail}' >"$out"
  log "  FAILED at $stage — recorded, continuing"
}

build_image() {
  [ -n "${SKIP_BUILD:-}" ] && return 0
  log "building $IMAGE (hermetic: no toolchains, no network at run time)"
  docker build --quiet -f "$HERE/Dockerfile.tropism" -t "$IMAGE" "$ROOT" >/dev/null
}

analyze() {
  local dir="$1" out="$2" started elapsed status
  started=$(date +%s)

  # --network none proves the offline claim rather than assuming it.
  # :ro proves tropism never writes into the tree it analyzes.
  set +e
  docker run --rm \
    --network none \
    --read-only \
    --tmpfs /tmp:rw,size=64m \
    --memory 4g \
    --pids-limit 512 \
    --mount "type=bind,source=$dir,target=/work,readonly" \
    "$IMAGE" analyze /work --format json --no-rules \
    >"$out.tmp" 2>"$out.stderr"
  status=$?
  set -e
  elapsed=$(( $(date +%s) - started ))

  # Exit 1 is "found something", not a failure. Anything above it is.
  if [ "$status" -gt 1 ] || ! jq -e . "$out.tmp" >/dev/null 2>&1; then
    record_failure "$out" "analyze (exit $status)" "$(tail -c 2000 "$out.stderr" 2>/dev/null)"
    rm -f "$out.tmp"
    return 0
  fi

  jq --argjson secs "$elapsed" --argjson exit "$status" \
     '. + {evaluation: {seconds: $secs, exit_code: $exit}}' \
     <"$out.tmp" >"$out"
  rm -f "$out.tmp" "$out.stderr"
  log "  ok in ${elapsed}s"
}

build_image
only="${1:-}"

while IFS=$'\t' read -r repo sha _langs _shape _note; do
  case "$repo" in \#*|"") continue ;; esac
  [ -n "$only" ] && [ "$repo" != "$only" ] && continue
  [ "$sha" = "UNPINNED" ] && { log "SKIP $repo (unpinned)"; continue; }

  slug="${repo//\//__}"
  out="$RESULTS/$slug.json"
  if [ -f "$out" ] && [ -z "${FORCE:-}" ]; then
    log "SKIP $repo (already analyzed)"
    continue
  fi

  disk_guard "$CHECKOUTS" || exit 1
  log "$repo @ ${sha:0:12}"

  dir="$CHECKOUTS/$slug"
  if ! fetch_repo "$repo" "$sha" "$dir"; then
    record_failure "$out" "clone" "git fetch or checkout failed for $sha"
    rm -rf "$dir"
    continue
  fi

  analyze "$dir" "$out"

  if [ -z "${KEEP:-}" ]; then
    freed=$(size_mib "$dir")
    rm -rf "$dir"
    log "  reclaimed $(human "${freed:-0}") ($(human "$(free_mib "$CHECKOUTS")") free)"
  fi
done < "$CORPUS"

failed=$(grep -l '"error"' "$RESULTS"/*.json 2>/dev/null | wc -l | tr -d ' ')
total=$(find "$RESULTS" -maxdepth 1 -name '*.json' | wc -l | tr -d ' ')
log "done — $total results, $failed failed"
[ "$failed" != "0" ] && log "failures are recorded in results/ and reported by ./report.py"
exit 0
