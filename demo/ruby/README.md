# Ruby demo

Planted problems:

- **Cycle** — `lib/shop/order.rb` and `lib/shop/invoice.rb` require each other.
  `require` is idempotent and returns `false` the second time, so this never
  raises at load; whichever file loads first wins, and the other sees a
  partially-defined constant somewhere unrelated.
- `awesome_print` is declared in the `Gemfile` and never required.
- `nokogiri` is required in `lib/shop/client.rb` and never declared.

Planted traps, which tropism must **not** report:

- `require "faraday/retry"` is either a file inside the `faraday` gem or the
  separate `faraday-retry` gem. Both are real; the `Gemfile` decides, and it
  declares `faraday`.
- `require "shop/order"` is a load-path require that resolves through `lib/` to
  this project's own file — not to a gem called `shop`.
- `rspec` is a `group :development, :test` gem used only from `spec/`, and a spec
  requiring the code under test is not a cycle.
- Stdlib requires (`json`) need no declaration.

## Dependency rules (`tropism.toml`)

- **Violated** — `entrypoint-goes-through-the-client`: `lib/shop.rb` requires the
  store directly.
- **Satisfied** — `store-is-the-bottom-layer`: the store requires nothing above it.
- **Violated** — `http-stays-in-the-client`: `faraday` is scoped to the client and
  `store.rb` uses it to warm a cache.

## The Gemfile is a program

Bundler evaluates the `Gemfile` in a DSL context, so a gem name can be computed.
tropism parses it with the Ruby grammar and takes the declarative subset — `gem`
calls and the `group` blocks around them. Anything dynamic contributes nothing:
`gem "rails-#{variant}"` names no gem that can be known without running the file,
and inventing one would put a package that does not exist into a report.

## Why both resolved-tree checks report clean

`version-conflict` and `diamond-dep` say `ok` here, and that is the answer rather
than a gap. Bundler resolves **flat** — one version of each gem for the whole
application — and refuses to write a lockfile it could not resolve that way. A
`Gemfile.lock` containing two versions of one gem does not exist, so the checks
that look for one will always find nothing. `Gemfile.lock` is nevertheless a
genuinely resolved tree, with exact versions and edges, which is more than
`go.sum` offers.
