import XCTest

@MainActor
final class CraxiiUITests: XCTestCase {
    private func identified(_ identifier: String, in app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)[identifier]
    }

    func testSetupSurfaceIsDiscoverable() {
        let app = XCUIApplication()
        app.launchEnvironment["CRAXII_STAGE22_UI_SMOKE"] = "setup"
        app.launch()
        XCTAssertTrue(app.windows["Craxii"].waitForExistence(timeout: 5))
        XCTAssertTrue(identified("setup.root", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(identified("setup.endpoint", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(identified("setup.credential", in: app).waitForExistence(timeout: 5))
    }

    func testConversationAndComposerSurfacesAreDiscoverable() {
        let app = XCUIApplication()
        app.launchEnvironment["CRAXII_STAGE22_UI_SMOKE"] = "conversation"
        app.launch()
        XCTAssertTrue(app.windows["Craxii"].waitForExistence(timeout: 5))
        XCTAssertTrue(identified("conversation.root", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(identified("conversation.transcript", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(identified("composer.editor", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(identified("composer.send", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(identified(
            "transcript.row.01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c03", in: app
        ).waitForExistence(timeout: 5))
        XCTAssertTrue(identified("work.cancel", in: app).waitForExistence(timeout: 5))
    }
}
