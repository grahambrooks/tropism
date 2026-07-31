#!/usr/bin/env bash
#
# A guided tour of the gdep CLI, run against a throwaway fixture repository.
#
#   ./scripts/demo.sh          # non-interactive walkthrough
#   ./scripts/demo.sh --tui    # ...then open the interactive browser
#
# Everything here runs against a temporary directory that is removed on exit; the
# script never touches the repository it lives in.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO/target/debug/gdep"
WITH_TUI=false
[[ "${1:-}" == "--tui" ]] && WITH_TUI=true

WORK="$(mktemp -d "${TMPDIR:-/tmp}/gdep-demo.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

if [[ -t 1 ]]; then
  B=$'\033[1m'; DIM=$'\033[2m'; CYAN=$'\033[36m'; R=$'\033[0m'
else
  B=""; DIM=""; CYAN=""; R=""
fi

step() { printf '\n%s══ %s %s\n' "$B" "$1" "$R"; }
note() { printf '%s%s%s\n' "$DIM" "$1" "$R"; }
run() {
  printf '\n%s$ %s%s\n' "$CYAN" "$*" "$R"
  "$@" || return $?
}
# Runs a command that is expected to influence the exit code, and reports it.
run_status() {
  printf '\n%s$ %s%s\n' "$CYAN" "$*" "$R"
  set +e
  "$@"
  local code=$?
  set -e
  printf '%sexit=%s%s\n' "$DIM" "$code" "$R"
  return 0
}

step "0. Build"
cargo build -q -p gdep-cli --manifest-path "$REPO/Cargo.toml"
note "built $BIN"

# ---------------------------------------------------------------------------
step "1. A fixture repository"

mkdir -p "$WORK/api" "$WORK/worker" "$WORK/vendor/thirdparty" "$WORK/node_modules/junk"

cat > "$WORK/.gitignore" <<'EOF'
vendor/
node_modules/
EOF

# Two real Go projects. `api` has a lockfile, `worker` does not — that difference
# is the point of step 4.
printf 'module example.com/api\n\ngo 1.24\n'    > "$WORK/api/go.mod"
printf 'github.com/spf13/cobra v1.8.0 h1:fake\n' > "$WORK/api/go.sum"
printf 'module example.com/worker\n\ngo 1.24\n' > "$WORK/worker/go.mod"

# Neither of these should ever be analyzed: both are ignored.
printf 'module example.com/vendored\n' > "$WORK/vendor/thirdparty/go.mod"
printf 'module example.com/junk\n'     > "$WORK/node_modules/junk/go.mod"

note "created:"
(cd "$WORK" && find . -name 'go.*' -o -name '.gitignore' | sort | sed 's/^/  /')

# ---------------------------------------------------------------------------
step "2. Discovery, and what --format auto does"

note "On a terminal, 'auto' gives human diagnostics. Forcing --format text shows"
note "the same thing regardless of where output is going."
run "$BIN" analyze "$WORK" --format text

note ""
note "Note what is NOT listed: vendor/ and node_modules/ were skipped because"
note ".gitignore says so — gdep honours it even outside a git repository."

# ---------------------------------------------------------------------------
step "3. The same run as JSON"

note "This is the contract in design/05-interfaces.md, and it is the exact"
note "serializer the MCP server uses — Report::to_json_pretty in gdep-core."
note "Piping automatically selects it, so CI needs no extra flag:"
run bash -c "'$BIN' analyze '$WORK' | head -32"
note "  ...(truncated)"

# ---------------------------------------------------------------------------
step "4. 'Zero findings' is not the same as 'checked and clean'"

note "Every check currently reports 'unavailable' because no analyzer is built yet."
note "That is deliberate: a consumer must never read silence as success. The same"
note "mechanism reports a missing lockfile once the analyzers land — 'worker' has"
note "no go.sum, so its resolved-tree checks would be unavailable for that reason."
run bash -c "'$BIN' analyze '$WORK' --format json | grep -c unavailable | sed 's/^/unavailable check count: /'"

# ---------------------------------------------------------------------------
step "5. Exit codes are the CI contract"

note "0 = ran, nothing at or above --fail-on"
note "1 = ran, findings at or above --fail-on"
note "2 = could not run at all"
note ""
note "No findings exist yet, so even the strictest threshold passes:"
run_status bash -c "'$BIN' analyze '$WORK' --fail-on info --format json > /dev/null"

note ""
note "A bad path is exit 2, never exit 1 — a broken invocation must not look"
note "like a passing build:"
run_status "$BIN" analyze "$WORK/does-not-exist" --format json

# ---------------------------------------------------------------------------
step "6. --no-ignore, for when you do want the vendored tree"

run bash -c "'$BIN' analyze '$WORK' --no-ignore --format json | grep '\"root\"'"
note "vendor/ and node_modules/ now appear."

# ---------------------------------------------------------------------------
step "7. The interactive browser"

if $WITH_TUI; then
  note "Opening --format tui. Keys: j/k or arrows to move, g/G first/last, q to quit."
  "$BIN" analyze "$WORK" --format tui
else
  note "Not shown here because it needs a terminal and takes over the screen."
  note "Re-run with --tui to open it, or try it directly:"
  printf '\n  %s%s analyze <path> --format tui%s\n' "$CYAN" "$BIN" "$R"
  note ""
  note "It refuses to run when redirected, rather than emitting escape codes:"
  run_status bash -c "'$BIN' analyze '$WORK' --format tui > /dev/null"
fi

# ---------------------------------------------------------------------------
step "Summary"
cat <<EOF
  --format text   rustc/clippy-style diagnostics with source snippets
  --format json   machine contract, shared verbatim with the MCP server
  --format tui    interactive browser (terminal only)
  --format auto   text on a tty, json when piped  [default]

  Working today: discovery, the report contract, all three renderers.
  Not built yet: every analyzer, which is why each check reports 'unavailable'.
  Next per design/07-open-questions.md: Go import extraction, then cycle detection.
EOF
