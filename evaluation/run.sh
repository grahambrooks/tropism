#!/usr/bin/env bash
#
# The tropism pass: clone the pinned corpus and analyze each repository inside the
# hermetic container.
#
# Resumable — a repository whose result already exists is skipped, so an
# interrupted run costs nothing to restart. Nothing here needs a toolchain on the
# host beyond git and docker.
#
#   ./run.sh                 # whole corpus
#   ./run.sh vercel/next.js  # one repository
#   FORCE=1 ./run.sh         # re-analyze even where a result exists
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
CORPUS="$HERE/corpus.tsv"
CHECKOUTS="${CHECKOUTS:-$HERE/.checkouts}"
RESULTS="$HERE/results"
IMAGE="${IMAGE:-tropism-eval}"

mkdir -p "$CHECKOUTS" "$RESULTS"

log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

build_image() {
  if [ -n "${SKIP_BUILD:-}" ]; then return; fi
  log "building $IMAGE (hermetic: no toolchains, no network at run time)"
  docker build --quiet -f "$HERE/Dockerfile.tropism" -t "$IMAGE" "$ROOT" >/dev/null
}

# Shallow, blobless clone at the pinned SHA. Blobless rather than `--depth 1`
# because a pinned SHA is often not the tip by the time this runs.
fetch() {
  local repo="$1" sha="$2" dir="$3"
  if [ -d "$dir/.git" ]; then
    if [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" = "$sha" ]; then return; fi
    rm -rf "$dir"
  fi
  mkdir -p "$dir"
  git -C "$dir" init --quiet
  git -C "$dir" remote add origin "https://github.com/$repo.git"
  git -C "$dir" fetch --quiet --depth 1 --filter=blob:none origin "$sha"
  git -C "$dir" checkout --quiet FETCH_HEAD
}

analyze() {
  local repo="$1" dir="$2" out="$3"
  local started elapsed status
  started=$(date +%s)

  # --network none proves the offline claim rather than assuming it.
  # :ro on the mount proves tropism never writes into the tree it analyzes.
  # --pids-limit and --memory bound a pathological repository.
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

  if [ "$status" -gt 1 ]; then
    log "  FAILED exit=$status (see $(basename "$out").stderr)"
    printf '{"error":"tropism exited %s","stderr":%s}\n' \
      "$status" "$(jq -Rs . <"$out.stderr")" >"$out"
    rm -f "$out.tmp"
    return
  fi

  # Wall-clock and exit code are part of the result, so D7/D8 need no second run.
  jq --argjson secs "$elapsed" --argjson exit "$status" \
     '. + {evaluation: {seconds: $secs, exit_code: $exit}}' \
     <"$out.tmp" >"$out"
  rm -f "$out.tmp"
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

  log "$repo @ ${sha:0:12}"
  fetch "$repo" "$sha" "$CHECKOUTS/$slug"
  analyze "$repo" "$CHECKOUTS/$slug" "$out"
done < "$CORPUS"

log "done — results in $RESULTS"
