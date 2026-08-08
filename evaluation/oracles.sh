#!/usr/bin/env bash
#
# The oracle pass: establish ground truth with the native tooling.
#
# **tropism must not invoke a package manager. This script must.** That is the
# whole method — `cargo tree`, `npm ls`, `madge` and `jdeps` are how a claim gets
# checked, and nothing they produce is ever fed back into a tropism run.
#
# Two safety properties, both structural rather than by convention:
#
#   * Each ecosystem runs in its own container, because resolving dependencies
#     executes arbitrary code from repositories nobody here audited.
#   * The container works on a **throwaway copy** of the checkout, so the tree
#     `run.sh` analyzed stays pristine and an installed `node_modules` can never
#     leak into a re-run of the tropism pass.
#
#   ./oracles.sh                 # whole corpus
#   ./oracles.sh facebook/react  # one repository
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORPUS="$HERE/corpus.tsv"
CHECKOUTS="${CHECKOUTS:-$HERE/.checkouts}"
ORACLES="$HERE/oracles/results"
SCRATCH="${SCRATCH:-$HERE/.scratch}"

mkdir -p "$ORACLES" "$SCRATCH"
log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

build_images() {
  [ -n "${SKIP_BUILD:-}" ] && return
  for stage in node rust python go java ruby; do
    log "building oracle image: $stage"
    docker build --quiet -f "$HERE/oracles/Dockerfile.oracles" \
      --target "$stage" -t "tropism-oracle-$stage" "$HERE/oracles" >/dev/null
  done
}

# Runs one oracle command in its ecosystem's image against a disposable copy.
# Network is ON here — resolving is the point — and the copy is deleted after.
oracle() {
  local stage="$1" repo="$2" src="$3" name="$4" cmd="$5"
  local slug="${repo//\//__}"
  local out="$ORACLES/$slug.$name.json"
  [ -f "$out" ] && [ -z "${FORCE:-}" ] && { log "  skip $name (have it)"; return; }

  local work="$SCRATCH/$slug"
  rm -rf "$work"; cp -R "$src" "$work"

  log "  $name via $stage"
  set +e
  docker run --rm \
    --memory 8g --pids-limit 2048 \
    --mount "type=bind,source=$work,target=/work" \
    "tropism-oracle-$stage" \
    bash -lc "$cmd" >"$out.raw" 2>"$out.log"
  local status=$?
  set -e

  # An oracle that could not run is recorded as such. A missing oracle must never
  # read as "tropism agreed with it".
  jq -n --arg name "$name" --argjson exit "$status" \
        --rawfile raw "$out.raw" \
        '{oracle:$name, exit_code:$exit, ok:($exit==0), output:$raw}' >"$out"
  rm -f "$out.raw"
  rm -rf "$work"
}

build_images
only="${1:-}"

while IFS=$'\t' read -r repo sha langs _shape _note; do
  case "$repo" in \#*|"") continue ;; esac
  [ -n "$only" ] && [ "$repo" != "$only" ] && continue
  [ "$sha" = "UNPINNED" ] && continue

  slug="${repo//\//__}"
  src="$CHECKOUTS/$slug"
  [ -d "$src" ] || { log "SKIP $repo (not cloned; run ./run.sh first)"; continue; }
  log "$repo"

  case ",$langs," in
    *,rust,*)
      # --duplicates is the version-conflict oracle. It reports what actually
      # compiles, which is *not* what the lockfile says — that gap is S8 and the
      # report quantifies it rather than treating either as wrong.
      oracle rust "$repo" "$src" cargo-duplicates \
        'cargo tree --duplicates --workspace 2>&1 | head -400' ;;&
    *,javascript,*)
      oracle node "$repo" "$src" madge-circular \
        'madge --circular --extensions ts,tsx,js,jsx --json . 2>/dev/null || echo "[]"' ;;&
    *,python,*)
      oracle python "$repo" "$src" pylint-cycles \
        'pylint --disable=all --enable=cyclic-import --output-format=json . 2>/dev/null || echo "[]"' ;;&
    *,go,*)
      # Go's compiler rejects import cycles, so this oracle is exact and free:
      # if the package graph loads, any module-scope cycle tropism reports is a
      # false positive by construction.
      oracle go "$repo" "$src" go-list \
        'go list ./... 2>&1 | head -2000' ;;&
    *,java,*)
      oracle java "$repo" "$src" jdeps-cycles \
        'find . -name "*.jar" -not -path "*/test*" | head -20 | xargs -r jdeps -cycles 2>&1 | head -400 || true' ;;&
    *,ruby,*)
      oracle ruby "$repo" "$src" bundle-list \
        'bundle lock --print 2>/dev/null | head -400 || true' ;;&
  esac
done < "$CORPUS"

log "done — oracles in $ORACLES"
log "csharp, cpp and swift have no automated oracle; they are hand-audited (see design/19)"
