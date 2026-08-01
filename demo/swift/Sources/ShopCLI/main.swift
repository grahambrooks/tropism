import Foundation

// VIOLATED: tropism.toml says the executable composes ShopCore and nothing else.
// This reaches into the store directly, and Package.swift declares the target
// dependency that lets it.
import ShopCore
import ShopStore

let store = OrderStore()
for order in store.all() {
    Audit.record(order)
    print(order.id)
}
