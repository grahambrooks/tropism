#!/usr/bin/env bash
#
# The oracle pass: establish ground truth with the native tooling.
#
# **tropism must not invoke a package manager. This script must.** That is the
# method — `cargo tree`, `npm ls`, `madge` and `jdeps` are how a claim gets checked,
# and nothing they produce is ever fed back into a tropism run.
#
# Each ecosystem runs in its own container, because resolving dependencies executes
# arbitrary code from repositories nobody here audited, and because it *mutates* the
# tree. This script therefore clones its own disposable copy and deletes it — along
# with whatever `node_modules` or `target` the oracle created — before moving on.
# `run.sh` deletes its checkouts too, so the two passes never share a tree and
# neither can contaminate the other.
#
# The cost is cloning twice. That is the right trade here: disk is the scarce
# resource and network is not.
#
#   ./oracles.sh                 # whole corpus
#   ./oracles.sh facebook/react  # one repository
#
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$HERE/lib.sh"

CORPUS="$HERE/corpus.tsv"
ORACLES="$HERE/oracles/results"
SCRATCH="${SCRATCH:-$HERE/.scratch}"
# Higher than run.sh: an oracle installs dependencies on top of the checkout.
MIN_FREE_MIB="${MIN_FREE_MIB:-16384}"

mkdir -p "$ORACLES" "$SCRATCH"

build_images() {
  [ -n "${SKIP_BUILD:-}" ] && return 0
  for stage in node rust python go java ruby; do
    log "building oracle image: $stage"
    docker build --quiet -f "$HERE/oracles/Dockerfile.oracles" \
      --target "$stage" -t "tropism-oracle-$stage" "$HERE/oracles" >/dev/null
  done
}

# Runs one oracle in its ecosystem's image. Network is on — resolving is the point.
oracle() {
  local stage="$1" slug="$2" work="$3" name="$4" cmd="$5"
  local out="$ORACLES/$slug.$name.json"
  [ -f "$out" ] && [ -z "${FORCE:-}" ] && { log "  skip $name (have it)"; return 0; }

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
  jq -n --arg name "$name" --argjson exit "$status" --rawfile raw "$out.raw" \
    '{oracle:$name, exit_code:$exit, ok:($exit==0), output:$raw}' >"$out"
  rm -f "$out.raw"
  [ "$status" -eq 0 ] || log "    (exit $status — recorded as unavailable)"
}

build_images
only="${1:-}"

while IFS=$'\t' read -r repo sha langs _shape _note; do
  case "$repo" in \#*|"") continue ;; esac
  [ -n "$only" ] && [ "$repo" != "$only" ] && continue
  [ "$sha" = "UNPINNED" ] && continue

  slug="${repo//\//__}"
  # Nothing to do if every oracle for this repository already exists.
  if [ -z "${FORCE:-}" ] && [ -n "$(find "$ORACLES" -name "$slug.*.json" -print -quit 2>/dev/null)" ]; then
    log "SKIP $repo (oracles present)"
    continue
  fi

  disk_guard "$SCRATCH" || exit 1
  log "$repo @ ${sha:0:12}"

  work="$SCRATCH/$slug"
  if ! fetch_repo "$repo" "$sha" "$work"; then
    log "  clone failed — skipping oracles for this repository"
    rm -rf "$work"
    continue
  fi

  # A polyglot repository needs *every* matching oracle, not the first — mastodon
  # is Ruby and JavaScript, ruff is Rust and Python. `case` with `;;&` expresses
  # that and is bash 4, which macOS does not ship; `has_language` is the portable
  # spelling and reads better besides.
  if has_language "$langs" rust; then
    # --duplicates reports what actually compiles, which is *not* what the
    # lockfile says. That gap is S8; the report quantifies it rather than
    # treating either side as wrong.
    oracle rust "$slug" "$work" cargo-duplicates \
      'cargo tree --duplicates --workspace 2>&1 | head -400'
  fi
  if has_language "$langs" javascript; then
    oracle node "$slug" "$work" madge-circular \
      'madge --circular --extensions ts,tsx,js,jsx --json . 2>/dev/null || echo "[]"'
  fi
  if has_language "$langs" python; then
    oracle python "$slug" "$work" pylint-cycles \
      'pylint --disable=all --enable=cyclic-import --output-format=json . 2>/dev/null || echo "[]"'
  fi
  if has_language "$langs" go; then
    # Go's compiler rejects import cycles, so this oracle is exact and free: if
    # the package graph loads, any module-scope cycle tropism reports is a false
    # positive by construction.
    oracle go "$slug" "$work" go-list \
      'go list ./... 2>&1 | head -2000'
  fi
  if has_language "$langs" java; then
    oracle java "$slug" "$work" jdeps-cycles \
      'find . -name "*.jar" -not -path "*/test*" | head -20 | xargs -r jdeps -cycles 2>&1 | head -400 || true'
  fi
  if has_language "$langs" ruby; then
    oracle ruby "$slug" "$work" bundle-list \
      'bundle lock --print 2>/dev/null | head -400 || true'
  fi

  # Delete the copy *and* everything the oracle installed into it.
  used=$(size_mib "$work")
  rm -rf "$work"
  log "  reclaimed $(human "${used:-0}") ($(human "$(free_mib "$SCRATCH")") free)"
done < "$CORPUS"

log "done — oracles in $ORACLES"
log "csharp, cpp and swift have no automated oracle; they are hand-audited (design/19)"
exit 0
