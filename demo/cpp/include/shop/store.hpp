#pragma once

#include <vector>

#include "shop/order.hpp"

namespace shop {

class Store {
public:
  explicit Store(const char* path);

  std::vector<Order> all() const;
  void warm_cache() const;

private:
  const char* path_;
};

}  // namespace shop
