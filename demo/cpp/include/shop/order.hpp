#pragma once

#include <string>

// One arm of the planted include cycle. Include guards make this compile — each
// header is expanded once — but the declarations are then order-dependent, and the
// build breaks for whoever includes invoice.hpp first.
#include "shop/invoice.hpp"

namespace shop {

class Invoice;

class Order {
public:
  Order(std::string id, long total_cents);

  const std::string& id() const { return id_; }
  long total_cents() const { return total_cents_; }
  Invoice invoice() const;

private:
  std::string id_;
  long total_cents_;
};

}  // namespace shop
