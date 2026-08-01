// TRAP: a translation unit including its own header. src/order.cpp and
// include/shop/order.hpp are one component, so this is a self-edge and never a
// cycle.
#include "shop/order.hpp"

#include <utility>

#include <fmt/format.h>

namespace shop {

Order::Order(std::string id, long total_cents)
    : id_(std::move(id)), total_cents_(total_cents) {}

std::string describe(const Order& order) {
  return fmt::format("{} ({}p)", order.id(), order.total_cents());
}

}  // namespace shop
