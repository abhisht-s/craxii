import XCTest
@testable import Craxii

@MainActor
final class CraxiiTests: XCTestCase {
    func testDiagnosticStoreStartsWithNoCanonicalOrDraftState() {
        let store = ConversationStore()
        XCTAssertTrue(store.snapshot.projection.messages.isEmpty)
        XCTAssertTrue(store.snapshot.projection.works.isEmpty)
        XCTAssertTrue(store.snapshot.drafts.isEmpty)
    }
}
