import Foundation
import Logging

public struct Order: Sendable {
    public let id: String
    public let totalCents: Int
    public let placedAt: Date

    public init(id: String, totalCents: Int, placedAt: Date = Date()) {
        self.id = id
        self.totalCents = totalCents
        self.placedAt = placedAt
    }
}

public enum Audit {
    static let log = Logger(label: "shop.core")

    public static func record(_ order: Order) {
        log.info("order \(order.id)")
    }
}
