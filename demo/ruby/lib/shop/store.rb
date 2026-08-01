# Persistence.
#
# VIOLATED: tropism.toml scopes `faraday` to the client layer, and this module calls
# out over HTTP to warm a cache.

require "pg"
require "faraday"

require_relative "order"

module Shop
  class Store
    def initialize(dsn)
      @db = PG.connect(dsn)
    end

    def find(id)
      row = @db.exec_params("SELECT id, total_cents FROM orders WHERE id = $1", [id]).first
      Order.new(row["id"], row["total_cents"].to_i)
    end

    def warm_cache!
      Faraday.get("https://example.invalid/warm")
    end
  end
end
