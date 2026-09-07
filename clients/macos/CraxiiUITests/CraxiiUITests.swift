import CryptoKit
import Foundation
import XCTest

@MainActor
final class CraxiiUITests: XCTestCase {
    private struct Stage26Setup: Decodable {
        let endpoint: String
        let credential: String
        let stateDirectory: String

        enum CodingKeys: String, CodingKey {
            case endpoint, credential
            case stateDirectory = "state_directory"
        }
    }

    private struct Stage26Turn: Decodable {
        let userMessageID: String
        let assistantMessageID: String
        let workID: String
        let assistantSHA256: String

        enum CodingKeys: String, CodingKey {
            case userMessageID = "user_message_id"
            case assistantMessageID = "assistant_message_id"
            case workID = "work_id"
            case assistantSHA256 = "assistant_sha256"
        }
    }

    private struct Stage26Observation: Encodable {
        let phase: String
        let rowIdentifier: String?
        let textSHA256: String?
        let optimisticSeen: Bool?
        let activitySeen: Bool?
        let reconnectingSeen: Bool?

        enum CodingKeys: String, CodingKey {
            case phase
            case rowIdentifier = "row_identifier"
            case textSHA256 = "text_sha256"
            case optimisticSeen = "optimistic_seen"
            case activitySeen = "activity_seen"
            case reconnectingSeen = "reconnecting_seen"
        }
    }

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
        let content = identified(
            "transcript.content.01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c03", in: app)
        XCTAssertTrue(content.waitForExistence(timeout: 5))
        XCTAssertEqual(content.value as? String, "Stage 22 smoke message")
        XCTAssertTrue(identified("work.cancel", in: app).waitForExistence(timeout: 5))
    }

    func testStage26LiveNativeLunaRestartRelaunchAndFollowUp() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["CRAXII_STAGE26_LIVE"] == "1" else {
            throw XCTSkip("Stage 26 live native acceptance is explicitly opt-in")
        }
        assertFixtureActivationAbsent(environment)
        let controller = try controllerURL(environment)
        let setup: Stage26Setup = try await get("setup", from: controller)
        let app = configuredStage26Application(
            mode: "CRAXII_STAGE26_LIVE", setup: setup)
        app.launch()
        app.activate()
        try await completeRealSetup(in: app, setup: setup)

        let canonical = "Inspect your machine and tell me what OS, CPU architecture, current directory, and Git version you have."
        let firstTransient = try submit(canonical, in: app)
        let first: Stage26Turn = try await poll("turn/first", from: controller, timeout: 8 * 60)
        let firstRow = try committedRow(first.assistantMessageID, in: app, timeout: 30)
        let firstDigest = digest(valueText(firstRow))
        XCTAssertEqual(firstDigest, first.assistantSHA256)
        XCTAssertEqual(
            count("transcript.row.\(first.userMessageID)", in: app), 1)
        try await observe(Stage26Observation(
            phase: "first_turn", rowIdentifier: firstRow.identifier,
            textSHA256: firstDigest, optimisticSeen: firstTransient.optimistic,
            activitySeen: firstTransient.activity, reconnectingSeen: nil), at: controller)

        try await post("backend/kill", to: controller)
        let reconnecting = waitForConnectionState(in: app, timeout: 20)
        XCTAssertTrue(reconnecting, "the live app never rendered its reconnecting state")
        try await post("backend/restart", to: controller)
        XCTAssertTrue(identified("composer.send", in: app).waitForExistence(timeout: 40))
        XCTAssertEqual(
            count("transcript.row.\(first.userMessageID)", in: app), 1)
        XCTAssertEqual(
            count("transcript.row.\(first.assistantMessageID)", in: app), 1)
        try await observe(Stage26Observation(
            phase: "restart_replay", rowIdentifier: firstRow.identifier,
            textSHA256: firstDigest, optimisticSeen: nil, activitySeen: nil,
            reconnectingSeen: reconnecting), at: controller)

        quitNormally(app)
        app.launch()
        app.activate()
        XCTAssertTrue(identified("composer.editor", in: app).waitForExistence(timeout: 40))
        let restored = try committedRow(first.assistantMessageID, in: app, timeout: 20)
        XCTAssertEqual(digest(valueText(restored)), first.assistantSHA256)
        XCTAssertEqual(
            count("transcript.row.\(first.userMessageID)", in: app), 1)
        try await observe(Stage26Observation(
            phase: "app_relaunch", rowIdentifier: restored.identifier,
            textSHA256: digest(valueText(restored)), optimisticSeen: nil,
            activitySeen: nil, reconnectingSeen: nil), at: controller)

        let followTransient = try submit("What Git version did you find?", in: app)
        let follow: Stage26Turn = try await poll("turn/follow-up", from: controller, timeout: 8 * 60)
        let followRow = try committedRow(follow.assistantMessageID, in: app, timeout: 30)
        let followDigest = digest(valueText(followRow))
        XCTAssertEqual(followDigest, follow.assistantSHA256)
        XCTAssertNotEqual(first.workID, follow.workID)
        XCTAssertEqual(
            count("transcript.row.\(follow.assistantMessageID)", in: app), 1)
        try await observe(Stage26Observation(
            phase: "follow_up", rowIdentifier: followRow.identifier,
            textSHA256: followDigest, optimisticSeen: followTransient.optimistic,
            activitySeen: followTransient.activity, reconnectingSeen: nil), at: controller)
        app.terminate()
        try await post("complete", to: controller)
    }

    func testStage26DeterministicNativeCancellationSmoke() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard environment["CRAXII_STAGE26_CANCELLATION"] == "1" else {
            throw XCTSkip("Stage 26 deterministic native cancellation is explicitly opt-in")
        }
        assertFixtureActivationAbsent(environment)
        let controller = try controllerURL(environment)
        let setup: Stage26Setup = try await get("setup", from: controller)
        let app = configuredStage26Application(
            mode: "CRAXII_STAGE26_CANCELLATION", setup: setup)
        app.launch()
        app.activate()
        try await completeRealSetup(in: app, setup: setup)
        let workRows = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH 'work.row.'"))
        let predecessorCount = workRows.count
        XCTAssertEqual(predecessorCount, 1)
        _ = try submit("Cancel this deterministic native smoke.", in: app)

        XCTAssertTrue(app.staticTexts["Queued"].waitForExistence(timeout: 30))
        let workRow = workRows.element(boundBy: predecessorCount)
        XCTAssertTrue(workRow.waitForExistence(timeout: 30))
        let workIdentifier = workRow.identifier
        XCTAssertTrue(workIdentifier.hasPrefix("work.row."))
        app.activate()
        let cancel = workRow.descendants(matching: .any)["work.cancel"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 30))
        cancel.click()
        XCTAssertTrue(app.staticTexts["Cancelled"].waitForExistence(timeout: 30))
        XCTAssertEqual(count(workIdentifier, in: app), 1)

        quitNormally(app)
        app.launch()
        app.activate()
        XCTAssertTrue(app.staticTexts["Cancelled"].waitForExistence(timeout: 40))
        XCTAssertEqual(count(workIdentifier, in: app), 1)
        try await observe(Stage26Observation(
            phase: "deterministic_cancellation", rowIdentifier: workIdentifier,
            textSHA256: nil, optimisticSeen: nil, activitySeen: true,
            reconnectingSeen: nil), at: controller)
        app.terminate()
        try await post("complete", to: controller)
    }

    private func assertFixtureActivationAbsent(_ environment: [String: String]) {
        XCTAssertNil(environment["CRAXII_STAGE22_UI_SMOKE"])
        XCTAssertNil(environment["CRAXII_STAGE21_UI_SMOKE"])
        XCTAssertNil(environment["CRAXII_STAGE21_INTEGRATION"])
        XCTAssertNil(environment["CRAXII_STAGE22_INTEGRATION"])
    }

    private func controllerURL(_ environment: [String: String]) throws -> URL {
        guard let text = environment["CRAXII_STAGE26_CONTROL_URL"],
              let url = URL(string: text), url.host == "127.0.0.1" else {
            XCTFail("Stage 26 requires its loopback in-memory controller")
            throw URLError(.badURL)
        }
        return url
    }

    private func configuredStage26Application(
        mode: String, setup: Stage26Setup
    ) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment[mode] = "1"
        app.launchEnvironment["CRAXII_STAGE26_STATE_DIR"] = setup.stateDirectory
        return app
    }

    private func count(_ identifier: String, in app: XCUIApplication) -> Int {
        app.descendants(matching: .any).matching(identifier: identifier).count
    }

    private func quitNormally(_ app: XCUIApplication) {
        app.activate()
        app.typeKey("q", modifierFlags: .command)
        let predicate = NSPredicate { _, _ in app.state == .notRunning }
        let expectation = XCTNSPredicateExpectation(predicate: predicate, object: nil)
        XCTAssertEqual(XCTWaiter.wait(for: [expectation], timeout: 15), .completed)
    }

    private func completeRealSetup(in app: XCUIApplication, setup: Stage26Setup) async throws {
        app.activate()
        XCTAssertTrue(app.windows["Craxii"].waitForExistence(timeout: 10))
        let endpoint = identified("setup.endpoint", in: app)
        let credential = identified("setup.credential", in: app)
        XCTAssertTrue(endpoint.waitForExistence(timeout: 10))
        XCTAssertTrue(credential.waitForExistence(timeout: 10))
        app.activate()
        replaceText(in: endpoint, with: setup.endpoint)
        app.activate()
        credential.click()
        typeTextInChunks(setup.credential, into: credential)
        app.activate()
        identified("setup.apply-endpoint", in: app).click()
        try await Task.sleep(for: .milliseconds(300))
        let save = identified("setup.save-credential", in: app)
        XCTAssertTrue(save.waitForExistence(timeout: 5))
        app.activate()
        save.click()
        let deadline = Date().addingTimeInterval(10)
        while save.isEnabled && Date() < deadline {
            try await Task.sleep(for: .milliseconds(50))
        }
        let connect = identified("setup.connect", in: app)
        XCTAssertTrue(connect.waitForExistence(timeout: 10))
        app.activate()
        connect.click()
        XCTAssertTrue(identified("composer.editor", in: app).waitForExistence(timeout: 40))
    }

    private func replaceText(in element: XCUIElement, with text: String) {
        element.click()
        element.typeKey("a", modifierFlags: .command)
        element.typeKey(.delete, modifierFlags: [])
        typeTextInChunks(text, into: element)
    }

    private func typeTextInChunks(_ text: String, into element: XCUIElement) {
        var start = text.startIndex
        while start < text.endIndex {
            let end = text.index(after: start)
            let character = String(text[start ..< end])
            switch character {
            case ":":
                element.typeKey(";", modifierFlags: .shift)
            case "?":
                element.typeKey("/", modifierFlags: .shift)
            default:
                let lowercase = character.lowercased()
                element.typeKey(
                    lowercase,
                    modifierFlags: lowercase == character ? [] : .shift)
            }
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            start = end
        }
    }

    private func submit(_ text: String, in app: XCUIApplication) throws
        -> (optimistic: Bool, activity: Bool)
    {
        app.activate()
        let editor = identified("composer.editor", in: app)
        XCTAssertTrue(editor.waitForExistence(timeout: 10))
        editor.click()
        typeTextInChunks(text, into: editor)
        let send = identified("composer.send", in: app)
        XCTAssertTrue(send.isEnabled)
        send.click()
        let optimistic = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH 'transcript.row.local-'"))
            .firstMatch.waitForExistence(timeout: 3)
        let activity = app.descendants(matching: .any).matching(
            NSPredicate(format: "identifier BEGINSWITH 'work.row.'"))
            .firstMatch.waitForExistence(timeout: 30)
        XCTAssertTrue(activity, "the native work activity surface was not observed")
        return (optimistic, activity)
    }

    private func committedRow(
        _ messageID: String, in app: XCUIApplication, timeout: TimeInterval
    ) throws -> XCUIElement {
        let row = identified("transcript.row.\(messageID)", in: app)
        guard row.waitForExistence(timeout: timeout) else {
            XCTFail("durable assistant row did not render")
            throw URLError(.timedOut)
        }
        XCTAssertEqual(row.label, "Craxii committed message")
        return row
    }

    private func valueText(_ element: XCUIElement) -> String {
        if let value = element.value as? String, !value.isEmpty {
            return value
        }
        let rowPrefix = "transcript.row."
        guard element.identifier.hasPrefix(rowPrefix) else { return "" }
        let messageID = element.identifier.dropFirst(rowPrefix.count)
        let content = element.descendants(matching: .staticText)[
            "transcript.content.\(messageID)"]
        if content.exists, let value = content.value as? String, !value.isEmpty {
            return value
        }
        return content.exists ? content.label : ""
    }

    private func digest(_ text: String) -> String {
        SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    private func waitForConnectionState(in app: XCUIApplication, timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        let banner = identified("connection.banner", in: app)
        while Date() < deadline {
            let rendered = "\(banner.label) \(banner.value ?? "")"
            if rendered.contains("Reconnecting") { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }
        return false
    }

    private func poll<T: Decodable>(
        _ path: String, from controller: URL, timeout: TimeInterval
    ) async throws -> T {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            do { return try await get(path, from: controller) as T }
            catch let error as URLError where error.code == .resourceUnavailable {
                try await Task.sleep(for: .milliseconds(250))
            }
        }
        throw URLError(.timedOut)
    }

    private func get<T: Decodable>(_ path: String, from controller: URL) async throws -> T {
        let (data, response) = try await URLSession.shared.data(
            from: controller.appendingPathComponent(path))
        guard let http = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
        guard http.statusCode == 200 else { throw URLError(.resourceUnavailable) }
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func post(_ path: String, to controller: URL) async throws {
        var request = URLRequest(url: controller.appendingPathComponent(path))
        request.httpMethod = "POST"
        let (_, response) = try await URLSession.shared.data(for: request)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 204)
    }

    private func observe(_ value: Stage26Observation, at controller: URL) async throws {
        var request = URLRequest(url: controller.appendingPathComponent("observation"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "content-type")
        request.httpBody = try JSONEncoder().encode(value)
        let (_, response) = try await URLSession.shared.data(for: request)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 204)
    }
}
