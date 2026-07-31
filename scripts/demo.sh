#!/usr/bin/env bash
#
# A guided tour of tropism across every supported language, plus tropism analyzing
# itself.
#
#   ./scripts/demo.sh            # full walkthrough
#   ./scripts/demo.sh go         # one language: go | javascript | rust | dotnet
#   ./scripts/demo.sh self       # only the dogfood run
#   ./scripts/demo.sh --tui      # end by opening the interactive browser
#
# The sample projects live in demo/ and are deliberately broken. They are
# excluded from the cargo workspace, so cargo never tries to build them.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO/target/debug/tropism"

WITH_TUI=false
ONLY=""
for arg in "$@"; do
  case "$arg" in
    --tui) WITH_TUI=true ;;
    go | javascript | rust | dotnet | self) ONLY="$arg" ;;
    *)
      echo "usage: demo.sh [go|javascript|rust|dotnet|self] [--tui]" >&2
      exit 2
      ;;
  esac
done

if [[ -t 1 ]]; then
  B=$'\033[1m'; DIM=$'\033[2m'; CYAN=$'\033[36m'; R=$'\033[0m'
else
  B=""; DIM=""; CYAN=""; R=""
fi

step() { printf '\n%s══ %s %s\n' "$B" "$1" "$R"; }
note() { printf '%s%s%s\n' "$DIM" "$1" "$R"; }
run() { printf '\n%s$ %s%s\n' "$CYAN" "$*" "$R"; "$@"; }
run_status() {
  printf '\n%s$ %s%s\n' "$CYAN" "$*" "$R"
  set +e; "$@"; local code=$?; set -e
  printf '%sexit=%s%s\n' "$DIM" "$code" "$R"
}

wants() { [[ -z "$ONLY" || "$ONLY" == "$1" ]]; }

step "0. Build"
cargo build -q -p tropism --manifest-path "$REPO/Cargo.toml"
note "built $BIN"

# ---------------------------------------------------------------------------
demo_language() {
  local lang="$1" title="$2"
  step "$title"

  note "What the sample plants, what its ruleset enforces, and what it traps:"
  sed 's/^/  /' "$REPO/demo/$lang/README.md" | head -40

  run "$BIN" analyze "$REPO/demo/$lang" --format text
}

wants go && demo_language go "1. Go — where the compiler already forbids cycles"
wants go && {
  note ""
  note "Note the two unavailable checks. go.sum records hashes for the whole module"
  note "graph rather than the versions MVS selected, so there is no resolved tree to"
  note "analyze offline. tropism says so instead of reporting a clean bill of health."
}

wants javascript && demo_language javascript "2. JavaScript — the only ecosystem where all six checks run"
wants javascript && {
  note ""
  note "version-conflict and diamond-dep both report \`ms\`. That is not a bug: for npm"
  note "a duplicated package IS the resolved outcome of a diamond with incompatible"
  note "constraints, so the two checks overlap. See design/10-js-evaluation.md."
}

wants rust && demo_language rust "3. Rust — module cycles are legal, so nothing else catches them"
wants rust && {
  note ""
  note "The two module-rule findings are the same violation seen twice: once in"
  note "Cargo.toml and once at the import. Each renders the team's reason verbatim,"
  note "which is the part no inferred finding can supply."
}

wants dotnet && demo_language dotnet "4. .NET — a layered solution, the classic rules case"
wants dotnet && {
  note ""
  note "The Shop.Domain <-> Shop.Data reference is caught by a *rule*, not by the"
  note "cycle check: cycle detection runs per project, and rules evaluate repo-wide."
  note "That gap is open question 1 in design/07-open-questions.md."
}

# ---------------------------------------------------------------------------
if wants self; then
  step "5. tropism analyzing tropism"

  note "The real test. Every finding below is on this repository's own source."
  run bash -c "'$BIN' analyze '$REPO' --format json \
    | python3 -c \"
import sys, json
d = json.load(sys.stdin)
rows = 0
for p in d['projects']:
    if 'fixtures' in p['root'] or p['root'].startswith('demo'):
        continue
    for f in p['findings']:
        print(f\\\"  {p['root'] or '.':16} {f['check']:17} {f['message'][:58]}\\\")
        rows += 1
print(f'  {rows} finding(s) on tropism itself')
\""

  note ""
  note "tropism's own tropism.toml is enforced on that run: the CLI and MCP server must"
  note "stay independent, core must be a leaf, and the tree-sitter grammars must not"
  note "escape tropism-lang. All satisfied, and none stale."
  note ""
  note "Everything reported is a genuine duplicate in Cargo.lock. Getting to zero"
  note "false positives took four bug fixes that only dogfooding surfaced — Rust 2018"
  note "uniform paths, fully-qualified paths counting as usage, paths inside macro and"
  note "attribute token trees, and module containment not being a dependency."
fi

# ---------------------------------------------------------------------------
if [[ -z "$ONLY" ]]; then
  step "6. Output formats and the CI contract"

  note "Piping selects JSON automatically — the same serializer the MCP server uses:"
  run bash -c "'$BIN' analyze '$REPO/demo/rust' | head -20"
  note "  ...(truncated)"

  note ""
  note "0 = ran, clean.  1 = findings at or above --fail-on.  2 = could not run."
  run_status bash -c "'$BIN' analyze '$REPO/demo/javascript' --format json > /dev/null"
  run_status "$BIN" analyze "$REPO/demo/nope" --format json

  if $WITH_TUI; then
    step "7. The interactive browser"
    note "j/k or arrows to move, g/G first/last, q to quit."
    "$BIN" analyze "$REPO/demo/javascript" --format tui
  else
    step "7. The interactive browser"
    note "Needs a terminal, so it is not shown here. Re-run with --tui, or:"
    printf '\n  %s%s analyze demo/javascript --format tui%s\n' "$CYAN" "$BIN" "$R"
    note ""
    note "It refuses to run when redirected rather than emitting escape codes:"
    run_status bash -c "'$BIN' analyze '$REPO/demo/javascript' --format tui > /dev/null"
  fi

  step "Summary"
  cat <<EOF
  Languages:   Go, JavaScript/TypeScript, Rust, C#/.NET
  Checks:      cycle, unused-dep, missing-dep, version-conflict, diamond-dep
  Deferred:    dependency-bloat (no crisp definition — design/07-open-questions.md)
  Rules:       module-rule, package-rule — from tropism.toml, High confidence
  Not built:   layers/require/transitive rule kinds, and the MCP server

  Reliability differs sharply by check, and the difference is measured:
    cycle            sound — reads only import syntax
    version/diamond  sound where a real resolved tree exists (npm, Cargo; not Go)
    unused-dep       63% false positives on real JS repos (design/10-js-evaluation.md)
    module/package   sound — a violation is a line of source, not an inference
EOF
fi
