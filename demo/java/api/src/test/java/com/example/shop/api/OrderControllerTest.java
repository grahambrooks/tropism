package com.example.shop.api;

// TRAP: junit is test-scope and imported only here, and a test importing the code
// under test is not a cycle — src/test/java is a separate compilation.

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.util.List;

import org.junit.jupiter.api.Test;

import com.example.shop.api.orders.Order;

class OrderControllerTest {
    @Test
    void listsIds() {
        assertEquals(List.of("x"), new OrderController().ids(List.of(new Order("x", 1000))));
    }
}
