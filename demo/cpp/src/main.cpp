#include <cstdio>

// VIOLATED: tropism.toml says the entrypoint goes through the store and nothing
// else. This reaches into the invoice component directly.
#include "shop/invoice.hpp"
#include "shop/store.hpp"

#include <spdlog/spdlog.h>

int main() {
  spdlog::set_level(spdlog::level::info);
  shop::Store store("shop.db");
  for (const auto& order : store.all()) {
    std::printf("%s\n", order.id().c_str());
  }
  return 0;
}
