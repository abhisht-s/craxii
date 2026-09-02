import XCTest

@MainActor
final class CraxiiUITests: XCTestCase {
    func testDiagnosticWindowLaunchesWithExpectedControls() {
        let app = XCUIApplication()
        app.launchEnvironment["CRAXII_STAGE21_UI_SMOKE"] = "1"
        app.launch()
        XCTAssertTrue(app.windows["Craxii Diagnostics"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.textFields["diagnostic.endpoint"].exists)
        XCTAssertTrue(app.secureTextFields["diagnostic.credential"].exists)
        XCTAssertTrue(app.buttons["diagnostic.connect"].exists)
        XCTAssertTrue(app.buttons["diagnostic.reset"].exists)
    }
}
