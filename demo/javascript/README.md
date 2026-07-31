# JavaScript / TypeScript demo

The only ecosystem here where **all six checks can run**: `package-lock.json` is a
genuinely resolved graph, so version-conflict and diamond-dep have real input.

Planted problems:

- A mutual import cycle between `src/utils/helper.js` and `src/utils/format.js`.
  File-level, which is where JS cycles actually live.
- `left-pad` is declared but never imported.
- `chalk` is imported but never declared.
- `ms` is installed twice (2.0.0 and 2.1.3) because `express` and `vitest`
  disagree — reported by both version-conflict and diamond-dep.

Planted traps, which gdep must **not** report:

- `node:fs` — a Node builtin needs no declaration.
- `@types/*` and script-invoked tools are classified as tooling, since nothing
  ever imports them.
