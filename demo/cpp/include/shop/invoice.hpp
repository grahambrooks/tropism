#pragma once

#include <string>

// The other arm of the planted include cycle.
#include "shop/order.hpp"

namespace shop {

class Order;

class Invoice {
public:
  explicit Invoice(const Order& order);

  Order reissue(std::string id) const;

private:
  long total_cents_;
};

}  // namespace shop
