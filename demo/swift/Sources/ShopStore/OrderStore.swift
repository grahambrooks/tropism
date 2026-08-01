import Foundation

// TRAP: the submodule form. `import struct ShopCore.Order` names the module
// ShopCore, and resolution must reduce it to that rather than to `ShopCore.Order`.
import struct ShopCore.Order

// PLANTED: imported and declared nowhere in Package.swift.
import Alamofire

// VIOLATED: tropism.toml scopes Logging to ShopCore.
import Logging

public final class OrderStore {
    private let log = Logger(label: "shop.store")
    private var rows: [Order] = []

    public init() {}

    public func all() -> [Order] {
        log.debug("\(rows.count) rows")
        return rows
    }

    public func sync(from url: URL) {
        AF.request(url).response { _ in }
    }
}
