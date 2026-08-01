# Java demo

Two projects, two build tools: `api/` is Maven and `worker/` is Gradle. One
ruleset covers both, because rules evaluate repo-wide.

Planted problems:

- **Cycle** — `com.example.shop.api.orders` and `com.example.shop.api.billing`
  import each other. `javac` compiles mutually-dependent packages without
  complaint, so nothing in the toolchain rejects it.
- `org.apache.commons:commons-lang3` is declared in `api/pom.xml` and never
  imported.
- `com.fasterxml.jackson.core:jackson-databind` is declared in
  `worker/build.gradle` and never imported.
- `com.google.guava:guava` is imported in `OrderStore.java` and declared nowhere.
  Its coordinate is `com.google.guava:guava` and its package is
  `com.google.common` — the import→package problem in its Java form.

Planted traps, which tropism must **not** report:

- `<dependencyManagement>` is a version catalogue. Its entries are not
  dependencies of the module that carries them, and counting them reports every
  one unused.
- `<parent>` carries `groupId` and `artifactId` too. They are the parent's, not
  this module's.
- `scope=runtime` (Maven) and `runtimeOnly` (Gradle) are on the classpath and
  never compiled against, so an absent import is expected.
- `scope=test` junit is imported only from `src/test/java`, and a test importing
  the code under test is not a cycle.
- `runtimeOnly "org.postgresql:postgresql:$postgresVersion"` has an interpolated
  version. The coordinate is real and is kept; only the version is dropped.
- `testImplementation libs.junit.jupiter` is a version-catalog reference that
  names nothing without a second file, so it contributes nothing.
- The `package` statement sits behind a licence header in `OrderController.java`.

## Dependency rules (`tropism.toml`)

- **Violated, twice** — `the-worker-consumes-events`: `Reconciler.java` imports
  `com.example.shop.api.orders`, and `build.gradle` declares the coordinate that
  lets it. The declaration and the import are separate findings, because a rule
  broken in a manifest is still broken — and here they are in two different build
  tools' file formats.
- **Violated** — `guava-stays-in-the-worker`: caught at the import even though the
  api never declares guava. A denylist matches the code, not just the manifest.

## Why the resolved-tree checks are unavailable

`version-conflict` and `diamond-dep` report `unavailable` for both projects, for
two different reasons that amount to the same thing:

- **Maven has no lockfile.** Not an optional one — none. The resolved tree exists
  only inside a `mvn` invocation.
- **`gradle.lockfile` is opt-in and edge-free.** When it exists it records the
  version each configuration selected, one coordinate per line, with no dependency
  relationships at all. It can no more answer a diamond question than `go.sum`
  can, so tropism declines rather than reporting `0 findings` about a graph it never
  had.

## Why `missing-dep` is weaker here than it looks

Maven puts a dependency's own dependencies on the compile classpath, so Java code
can import a transitive artifact and compile cleanly. An import matching no
declared coordinate is therefore more often a transitive reach than a genuine
omission. tropism reports one only when it can name the coordinate — from the groupId
convention, or from the curated table that knows guava's package — and leaves
anything else unresolved, which caps hygiene confidence rather than inventing a
finding with no artifactId in it.
