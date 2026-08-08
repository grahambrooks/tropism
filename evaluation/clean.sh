#!/usr/bin/env bash
#
# Reclaim disk. Everything here is reproducible from `corpus.tsv`.
#
#   ./clean.sh            # clones and oracle scratch — keeps results
#   ./clean.sh --all      # results too: the next run starts from nothing
#   ./clean.sh --images   # also the docker images this harness built
#
# `run.sh` and `oracles.sh` already delete each tree as they finish with it, so
# this is for an interrupted run, or for reclaiming the results of a finished one.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$HERE/lib.sh"

targets=("$HERE/.checkouts" "$HERE/.scratch")
[ "${1:-}" = "--all" ] && targets+=("$HERE/results" "$HERE/oracles/results")

before=$(free_mib "$HERE")
for target in "${targets[@]}"; do
  [ -d "$target" ] || continue
  log "removing $(basename "$target") ($(human "$(size_mib "$target")"))"
  rm -rf "$target"
done

if [ "${1:-}" = "--images" ]; then
  for image in tropism-eval tropism-oracle-node tropism-oracle-rust \
               tropism-oracle-python tropism-oracle-go tropism-oracle-java \
               tropism-oracle-ruby; do
    docker image rm -f "$image" >/dev/null 2>&1 && log "removed image $image" || true
  done
  # Only this harness's build cache, never a blanket `docker system prune`, which
  # would take images that have nothing to do with the evaluation.
  docker builder prune --force --filter until=1h >/dev/null 2>&1 || true
fi

after=$(free_mib "$HERE")
log "free: $(human "$before") -> $(human "$after")"
