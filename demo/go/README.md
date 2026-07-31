# Go demo

Planted problems:

- `golang.org/x/sync` is declared in `go.mod` but never imported.
- `github.com/rs/zerolog` is imported in `api/api.go` but never declared.

Planted traps, which gdep must **not** report:

- `_ "github.com/lib/pq"` — a blank import is a real use, for its side effects.
- `github.com/stretchr/testify // indirect` — not expected to be imported.

## Dependency rules (`gdep.toml`)

- **Violated** — `entrypoint-goes-through-the-api`: `main.go` reaches past the api
  layer straight into `store`. No general check can state this; it is true only
  because the team decided the layering.
- **Satisfied** — `storage-is-the-bottom-layer`: `store` depends on nothing above it.
- **Violated** — `approved-dependencies`: the ruleset is closed-world
  (`unlisted = "deny"`), and `golang.org/x/sync` is not on the list.

Structurally unavailable for Go: `version-conflict` and `diamond-dep`. `go.sum`
records hashes for the whole module graph rather than the versions MVS selected,
so there is no resolved tree to analyze offline.
