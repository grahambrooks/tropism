package com.example.shop.api.billing;

// The other arm of the planted package cycle.
import com.example.shop.api.orders.Order;

public record Invoice(Order order) {
    public Order reissue(String id) {
        return new Order(id, order.totalCents());
    }
}
