# The other arm of the planted cycle.

require_relative "order"

module Shop
  class Invoice
    def initialize(order)
      @order = order
    end

    def reissue(id)
      Order.new(id, @order.total_cents)
    end
  end
end
