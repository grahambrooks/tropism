# One arm of the planted cycle: order requires invoice, invoice requires order.
#
# Ruby's require is idempotent and returns false the second time, so the cycle does
# not raise. It resolves to whichever file was loaded first winning, and the other
# seeing a partially defined constant — a NameError somewhere unrelated.

require_relative "invoice"

module Shop
  class Order
    attr_reader :id, :total_cents

    def initialize(id, total_cents)
      @id = id
      @total_cents = total_cents
    end

    def invoice
      Invoice.new(self)
    end
  end
end
