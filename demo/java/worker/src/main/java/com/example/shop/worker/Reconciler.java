package com.example.shop.worker;

import java.util.List;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

// VIOLATED: tropism.toml says the worker consumes events and must not reach into the
// api's classes. Caught at both levels — this import, and the coordinate declared
// in build.gradle.
import com.example.shop.api.orders.Order;

public final class Reconciler {
    private static final Logger LOG = LoggerFactory.getLogger(Reconciler.class);

    public long total(List<Order> orders) {
        LOG.info("reconciling {}", orders.size());
        return orders.stream().mapToLong(Order::totalCents).sum();
    }
}
