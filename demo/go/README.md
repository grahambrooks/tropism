# Go demo

Planted problems:

- `golang.org/x/sync` is declared in `go.mod` but never imported.
- `github.com/rs/zerolog` is imported in `api/api.go` but never declared.

Planted traps, which gdep must **not** report:

- `_ "github.com/lib/pq"` — a blank import is a real use, for its side effects.
- `github.com/stretchr/testify // indirect` — not expected to be imported.

Structurally unavailable for Go: `version-conflict` and `diamond-dep`. `go.sum`
records hashes for the whole module graph rather than the versions MVS selected,
so there is no resolved tree to analyze offline.
