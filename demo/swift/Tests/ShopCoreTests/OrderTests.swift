import XCTest

// TRAP: `@testable import` is a test target importing the module under test. That
// is what test targets are for, and it is never a cycle.
@testable import ShopCore

final class OrderTests: XCTestCase {
    func testKeepsItsTotal() {
        XCTAssertEqual(Order(id: "x", totalCents: 1000).totalCents, 1000)
    }
}
