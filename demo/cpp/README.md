# C++ demo

Conan is the package manager here (`conanfile.txt`). The `conanfile.py` and
`vcpkg.json` parsers are exercised by unit tests in `crates/tropism-lang/src/cpp.rs`.

Planted problems:

- **Cycle** — `include/shop/order.hpp` and `include/shop/invoice.hpp` include each
  other. Include guards make this *compile* — each header expands once — but the
  declarations become order-dependent, and the build breaks for whoever includes
  `invoice.hpp` first.
- `nlohmann_json` is declared in `conanfile.txt` and never included.
- `sqlite3` is included in `src/store.cpp` and declared nowhere.

Planted traps, which tropism must **not** report:

- `src/order.cpp` includes its own `include/shop/order.hpp`. Those are one
  component, so it is a self-edge and never a cycle.
- `cmake` is a `[tool_requires]` build tool and `gtest` a `[test_requires]` entry
  used only from `tests/`; neither is expected to be included from library code.
- `[generators]` and `[layout]` are not dependency lists.
- `<vector>`, `<string>`, `<cstdio>` are standard headers, recognised by shape
  rather than by a list. `<sys/stat.h>` is POSIX.

## Dependency rules (`tropism.toml`)

- **Violated** — `the-entrypoint-goes-through-the-store`: `main.cpp` includes
  `shop/invoice.hpp` directly.
- **Violated** — `logging-is-configured-at-the-entrypoint`: `spdlog` is scoped to
  the entrypoint and `store.cpp` logs through it.

Note the shape of the `[modules]` table: a C++ module names both the header and
the translation unit, because they are one component. Mapping only the header
would leave `store.cpp` in no module, and a rule about the store would then say
nothing about the code implementing it.

## A module is a component, not a file

`include/shop/order.hpp` and `src/order.cpp` are two files of one thing, and
`#include "shop/order.hpp"` names the header wherever it sits on the include path.
So tropism strips the include-path root (`include/`, `src/`, …) and the extension:
both files become the module `shop/order`.

Without that, every component would appear in the graph twice and a translation
unit including its own header would be an edge between two nodes — turning the
most ordinary line in C++ into a finding.

## Why the resolved-tree checks are unavailable

Neither ecosystem records a resolved tree. `conan.lock` is a flat list of pinned
references; vcpkg pins a *registry baseline commit* rather than a dependency
graph. Neither carries edges, so neither can answer a diamond question — the same
position as `go.sum`, `gradle.lockfile`, and `Package.resolved`.
