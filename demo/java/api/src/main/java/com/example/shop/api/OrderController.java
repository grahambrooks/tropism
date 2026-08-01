/*
 * TRAP: the package statement sits behind a licence header, and the module is the
 * package rather than the directory.
 */

package com.example.shop.api;

import java.util.List;

import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

import com.example.shop.api.orders.Order;

public final class OrderController {
    private static final Logger LOG = LoggerFactory.getLogger(OrderController.class);

    public List<String> ids(List<Order> orders) {
        LOG.info("listing {} orders", orders.size());
        return orders.stream().map(Order::id).toList();
    }
}
