# Python demo

A src-layout project (`src/app/…`), so every module here is `app.something` and
`src` never appears in an import.

Planted problems:

- **Cycle** — `app.orders` imports `app.billing`, which imports `app.orders`.
  Python permits this at import time and fails only when one half is imported
  first and finds the other half-initialised, so nothing in the toolchain rejects
  it.
- `rich` is declared in `pyproject.toml` and never imported.
- `httpx` is imported in `src/app/api.py` and never declared.
- `urllib3` is locked at two versions in `uv.lock`, because the resolution forked
  on interpreter version.

Planted traps, which tropism must **not** report:

- `import yaml` is the `PyYAML` distribution. The import name and the package name
  share nothing, so a tool that compares them literally reports `PyYAML` unused
  *and* `yaml` missing — two findings, both wrong, about one correct line.
- `pytest` is a dependency group entry used only from `tests/`, and a test module
  importing the code under test is not a cycle.
- `from .models import Order` is relative and resolves to `app.models`, not to a
  package called `models`.
- Stdlib imports (`os`, `json`, `dataclasses`, `datetime`, `sys`) need no
  declaration.

## Dependency rules (`tropism.toml`)

- **Violated** — `entrypoint-goes-through-the-api`: `main.py` imports
  `app.storage` directly.
- **Satisfied** — `storage-is-the-bottom-layer`: storage imports nothing above it.
- **Violated** — `http-calls-live-in-the-api`: `requests` is scoped to the api
  layer and `storage.py` uses it.

## What Python's flat environment means for the resolved tree

`pip` installs exactly one version of each distribution, so neither `uv.lock` nor
`poetry.lock` has a way to say *this copy* of a package — an edge names a
distribution and nothing more. When the resolution forks, as `urllib3` does here,
the same name appears twice and an edge naming it is genuinely ambiguous; tropism
drops those edges rather than attaching them to an arbitrary copy.

The consequence is visible above: `version-conflict` fires and `diamond-dep`
reports clean. That is the correct answer rather than a gap. A diamond is two
dependents forcing two *installed* copies, and a flat environment cannot have
them — what it has instead is one version that some dependent is not getting.
