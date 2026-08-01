# The entrypoint the load path finds first: `require "shop"`.
#
# VIOLATED: tropism.toml says the entrypoint composes the client and nothing else.
# This file requires the store directly.
#
# TRAP: `require "shop/order"` is a load-path require, not a relative one, and it
# resolves through lib/ to this project's own file — never to a gem called `shop`.

require "shop/order"

require_relative "shop/client"
require_relative "shop/store"

module Shop
  def self.lookup(id, dsn:, url:)
    Store.new(dsn).find(id) || Client.new(url).fetch(id)
  end
end
