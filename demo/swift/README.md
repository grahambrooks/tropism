# Swift demo

Planted problems:

- `swift-collections` is declared in `Package.swift` and no target takes a product
  from it.
- `Alamofire` is imported in `Sources/ShopStore/OrderStore.swift` and declared
  nowhere.

Planted traps, which tropism must **not** report:

- `import struct ShopCore.Order` names the module `ShopCore`, not a package called
  `ShopCore.Order`.
- `@testable import ShopCore` is a test target importing the module under test.
  That is what test targets are for, and it is never a cycle.
- `import Logging` is the `swift-log` package. Nothing in either name implies the
  other — see below.
- `Package.swift` is itself a `.swift` file that tropism walks, and it imports
  `PackageDescription`. So do `Foundation` and `XCTest`: all toolchain modules.

## Dependency rules (`tropism.toml`)

- **Violated** — `the-executable-composes-core`: `main.swift` imports `ShopStore`.
- **Satisfied** — `core-is-the-bottom-module`: `ShopCore` imports nothing above it.
- **Violated** — `logging-is-configured-once`: `Logging` is scoped to `ShopCore`
  and `OrderStore.swift` uses it.

## The one ecosystem that answers the import→package problem itself

Every other language here needs a curated table to know that `import yaml` is
`PyYAML`, or that `com.google.common` is `com.google.guava:guava`. Swift does not,
because the manifest states the mapping in the target that uses it:

```swift
.target(name: "ShopCore", dependencies: [
    .product(name: "Logging", package: "swift-log"),
])
```

So tropism records a dependency under its **product** name — what `import` actually
writes — and needs no guesswork. The exception is a package no target takes a
product from: that keeps its own identity (`swift-collections` above), because
"declared and used by nothing" is exactly what `unused-dep` is for.

## Why there is no cycle to find

`cycle` reports `ok` here, and no cycle is planted, for the same reason as the Go
demo: SwiftPM rejects a cyclic target dependency outright, and files within a
module do not import each other at all. A Swift cycle can only exist in a package
that does not build, so the check can only ever fire on code the toolchain has
already refused.

## Why the resolved-tree checks are unavailable

`Package.resolved` pins one version per package and records no dependency edges at
all — the same shape as `gradle.lockfile`. SwiftPM also resolves to a single
version per package, so there is neither a conflict to find nor a graph to
traverse for a diamond. tropism says so rather than reporting `0 findings` about a
tree it never had.
