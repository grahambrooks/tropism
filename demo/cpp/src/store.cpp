#include "shop/store.hpp"

#include <vector>

// PLANTED: sqlite3 is included and declared nowhere in conanfile.txt.
#include <sqlite3.h>

// VIOLATED: tropism.toml scopes spdlog to the entrypoint.
#include <spdlog/spdlog.h>

// TRAP: a POSIX header needs no package.
#include <sys/stat.h>

namespace shop {

Store::Store(const char* path) : path_(path) {}

std::vector<Order> Store::all() const {
  sqlite3* db = nullptr;
  sqlite3_open(path_, &db);
  spdlog::debug("opened {}", path_);
  return {};
}

void Store::warm_cache() const {
  struct stat info {};
  ::stat(path_, &info);
}

}  // namespace shop
