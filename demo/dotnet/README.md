# .NET demo

A four-project layered solution — the classic NDepend/JDepend shape, where the
constraint that matters is which project may reference which.

Two things here differ from every other demo:

- The manifest is named after the project (`Shop.Api.csproj`), not by convention.
- A `using` names a **namespace**, not a package. `using Xunit;` comes from the
  `xunit` package, `using Shop.Domain.Orders;` is the solution's own code, and
  telling them apart needs the set of namespaces the projects declare.

## Dependency rules (`gdep.toml`)

- **Violated** — `api-goes-through-the-domain`: `Shop.Api` reaches straight into
  `Shop.Data`. Caught **twice**: at the `<ProjectReference>` in the `.csproj` and
  at the `using` in `OrderController.cs`.
- **Violated** — `data-is-the-bottom-layer`: `Shop.Data` references
  `Shop.Domain`, inverting the layering. The C# compiler is perfectly happy —
  it only forbids cycles between *assemblies*, and this reference graph is acyclic.
- **Violated** — `one-logging-abstraction`: `Serilog` is banned. Caught at the
  `using` even though `Shop.Api` never declares it, because a denylist matches
  the code and not just the manifest.
- **Satisfied** — `sql-stays-in-the-data-layer` and
  `test-frameworks-stay-in-tests`.

## Planted problems

- A namespace cycle between `Shop.Domain.Orders` and `Shop.Domain.Billing`.
  C# permits namespace cycles, so nothing in the toolchain catches this.
- `AutoMapper` is declared by `Shop.Data` but never imported.
- `Serilog` is imported by `Shop.Api` but never declared.

## Planted traps, which gdep must **not** report

- `StyleCop.Analyzers` carries `PrivateAssets="all"` — a Roslyn analyzer that
  participates in the build and is never referenced from code.
- `global using System;` and the whole `System.*` namespace need no reference.
- `Shop.Domain.Tests` is a separate assembly that references the code under
  test, so it can never form a cycle with it.

## What is unavailable, and why

`packages.lock.json` is opt-in in .NET (`RestorePackagesWithLockFile`) and is
absent here, as it is in most real solutions. So `version-conflict` and
`diamond-dep` report **unavailable** rather than clean — the same position Java
will land in, and the price of never invoking a package manager.

## A limitation this demo makes visible

The `Shop.Domain` ↔ `Shop.Data` reference is caught by a **rule**, not by
`cycle`. Cycle detection runs per project, so a cycle spanning two projects is
invisible to it. That is the repo-wide graph gap recorded as open question 1 in
`design/07-open-questions.md`; rules already evaluate repo-wide, which is why
they catch what the cycle check cannot.
