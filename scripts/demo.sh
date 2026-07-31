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

mkdir -p "$WORK/api" "$WORK/store" "$WORK/worker" "$WORK/vendor/thirdparty" "$WORK/node_modules/junk"

cat > "$WORK/.gitignore" <<'EOF'
vendor/
node_modules/
EOF

# A Go module with three deliberate problems and three deliberate traps.
cat > "$WORK/go.mod" <<'EOF'
module example.com/shop

go 1.24

require (
	github.com/spf13/cobra v1.8.0
	github.com/google/uuid v1.6.0
	golang.org/x/sync v0.7.0
	github.com/lib/pq v1.10.9
	github.com/stretchr/testify v1.9.0 // indirect
)
EOF
printf 'github.com/spf13/cobra v1.8.0 h1:fake\n' > "$WORK/go.sum"

cat > "$WORK/main.go" <<'EOF'
package main

import (
	"fmt"

	"github.com/spf13/cobra"

	"example.com/shop/api"
)

func main() {
	cmd := &cobra.Command{Use: "shop"}
	fmt.Println(api.Name, cmd.Use)
}
EOF

cat > "$WORK/api/api.go" <<'EOF'
package api

import (
	"github.com/google/uuid"
	"github.com/rs/zerolog"

	"example.com/shop/store"
)

var Name = "api"

func New() string {
	zerolog.Nop().Info().Msg("new")
	return uuid.New().String() + store.Driver
}
EOF

cat > "$WORK/store/store.go" <<'EOF'
package store

import (
	_ "github.com/lib/pq"
)

var Driver = "postgres"
EOF

# A second module without a lockfile, to contrast check availability.
printf 'module example.com/worker\n\ngo 1.24\n' > "$WORK/worker/go.mod"
printf 'package worker\n\nimport "fmt"\n\nfunc Run() { fmt.Println("work") }\n' > "$WORK/worker/worker.go"

# Neither of these should ever be analyzed: both are ignored.
printf 'module example.com/vendored\n' > "$WORK/vendor/thirdparty/go.mod"
printf 'module example.com/junk\n' > "$WORK/node_modules/junk/go.mod"

note "created:"
(cd "$WORK" && find . \( -name 'go.*' -o -name '*.go' \) | sort | sed 's/^/  /')
note ""
note "Planted problems:  golang.org/x/sync declared but never imported"
note "                   github.com/rs/zerolog imported but never declared"
note "Planted traps:     _ \"github.com/lib/pq\"  (blank import — is a real use)"
note "                   testify // indirect     (not expected to be imported)"

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

note "Three checks report 'unavailable' rather than passing silently. Two need a"
note "resolved dependency tree, and go.sum is not one: it records hashes for the"
note "whole module graph, not the versions MVS selected, and carries no edges."
note "Saying so beats reporting a clean bill of health gdep cannot actually give."
run bash -c "'$BIN' analyze '$WORK' --format json | python3 -c \"
import sys, json
for p in json.load(sys.stdin)['projects']:
    for check, s in p['checks'].items():
        if s['status'] == 'unavailable':
            print(f\\\"  {p['root'] or '.':8} {check:18} {s['reason'][:60]}\\\")
\""

# ---------------------------------------------------------------------------
step "5. Exit codes are the CI contract"

note "0 = ran, nothing at or above --fail-on"
note "1 = ran, findings at or above --fail-on"
note "2 = could not run at all"
note ""
note "The planted missing dependency is an error, so the default threshold fails:"
run_status bash -c "'$BIN' analyze '$WORK' --format json > /dev/null"

note ""
note "Raising the threshold above every finding passes again:"
run_status bash -c "'$BIN' analyze '$WORK' --fail-on error --format json | python3 -c \"
import sys, json
d = json.load(sys.stdin)
print('  findings:', sum(len(p['findings']) for p in d['projects']))
\""

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

  Working today: the full Go slice — discovery, go.mod parsing, tree-sitter
                 import extraction, module graph, and three of six checks.
  Unavailable:   version-conflict and diamond (need a resolved tree Go cannot
                 give offline), and dependency-bloat (deferred by design).
  Not built yet: the other nine languages, and the MCP server.
EOF
