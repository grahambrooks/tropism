package com.example.shop.api.storage;

import java.util.List;

// PLANTED: guava is imported and never declared. Its coordinate is
// com.google.guava:guava and its package is com.google.common — nothing in either
// name implies the other, which is the whole import→package problem in Java.
//
// VIOLATED: tropism.toml scopes guava to the worker.
import com.google.common.collect.ImmutableList;

import com.example.shop.api.orders.Order;

public final class OrderStore {
    private final List<Order> rows = ImmutableList.of();

    public List<Order> all() {
        return rows;
    }
}
