// TRAP: a test including the code under test is not a cycle, and gtest is a
// [test_requires] entry used only here.

#include <gtest/gtest.h>

#include "shop/order.hpp"

TEST(OrderTest, KeepsItsTotal) {
  shop::Order order("x", 1000);
  EXPECT_EQ(order.total_cents(), 1000);
}
