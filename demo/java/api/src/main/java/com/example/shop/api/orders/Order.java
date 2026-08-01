package com.example.shop.api.orders;

// One arm of the planted package cycle. javac compiles mutually-dependent packages
// without complaint, so nothing in the Java toolchain rejects this.
import com.example.shop.api.billing.Invoice;

public record Order(String id, long totalCents) {
    public Invoice invoice() {
        return new Invoice(this);
    }
}
