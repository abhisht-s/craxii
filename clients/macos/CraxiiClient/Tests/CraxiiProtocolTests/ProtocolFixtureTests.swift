import Foundation
import CryptoKit
import Testing
@testable import CraxiiProtocol

private let repositoryRoot: URL = {
    var url = URL(fileURLWithPath: #filePath)
    for _ in 0..<6 { url.deleteLastPathComponent() }
    return url
}()

private let fixtureDirectory = repositoryRoot
    .appendingPathComponent("backend/tests/fixtures/protocol-v1", isDirectory: true)

private func fixture(_ name: String) throws -> Data {
    try Data(contentsOf: fixtureDirectory.appendingPathComponent(name))
}

@Test func checkedManifestIsConsumedAndEveryFixtureHashMatches() throws {
    let manifest = try String(contentsOf: fixtureDirectory.appendingPathComponent("manifest.sha256"), encoding: .utf8)
    let rows = manifest.split(separator: "\n")
    #expect(rows.count == 10)
    for row in rows {
        let fields = row.split(separator: " ", omittingEmptySubsequences: true)
        #expect(fields.count == 2)
        let data = try fixture(String(fields[1]))
        let hash = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        #expect(hash == fields[0])
    }
}

@Test func allPublicProtocolFixturesDecodeThroughProductionModels() throws {
    let decoder = JSONDecoder()
    _ = try decoder.decode(MessageRequest.self, from: fixture("message-request.json"))
    _ = try decoder.decode(MessageReceipt.self, from: fixture("message-response.json"))
    _ = try decoder.decode(CancellationRequest.self, from: fixture("cancellation-request.json"))
    _ = try decoder.decode(CancellationReceipt.self, from: fixture("cancellation-response.json"))
    _ = try decoder.decode(BootstrapResponse.self, from: fixture("bootstrap-snapshot.json"))
    _ = try decoder.decode(BackendErrorEnvelope.self, from: fixture("error-envelope.json"))
    _ = try decoder.decode(HealthResponse.self, from: fixture("health.json"))
    _ = try decoder.decode([DurableEventEnvelope].self, from: fixture("durable-events.json"))
    _ = try decoder.decode([EphemeralDraftEnvelope].self, from: fixture("ephemeral-drafts.json"))
    guard case .syncComplete = try ServerFrame.decode(fixture("sync-complete.json")) else {
        Issue.record("sync fixture did not decode as sync.complete")
        return
    }
}

@Test func additiveResponseFieldsAreIgnoredButRequiredFieldsAndEnumsRemainStrict() throws {
    let source = try JSONSerialization.jsonObject(with: fixture("bootstrap-snapshot.json"))
    var object = try #require(source as? [String: Any])
    object["future_optional_field"] = ["nested": true]
    _ = try JSONDecoder().decode(
        BootstrapResponse.self, from: JSONSerialization.data(withJSONObject: object))

    object.removeValue(forKey: "snapshot_cursor")
    #expect(throws: Error.self) {
        _ = try JSONDecoder().decode(
            BootstrapResponse.self, from: JSONSerialization.data(withJSONObject: object))
    }

    var health = try #require(JSONSerialization.jsonObject(with: fixture("health.json")) as? [String: Any])
    health["status"] = "future_status"
    #expect(throws: Error.self) {
        _ = try JSONDecoder().decode(HealthResponse.self, from: JSONSerialization.data(withJSONObject: health))
    }
}

@Test func protocolVersionCursorAndUUIDv7ValidationFailClosed() throws {
    var request = try #require(
        JSONSerialization.jsonObject(with: fixture("message-request.json")) as? [String: Any])
    request["protocol_version"] = 2
    #expect(throws: Error.self) {
        _ = try JSONDecoder().decode(MessageRequest.self, from: JSONSerialization.data(withJSONObject: request))
    }
    for invalid in [
        "01890F3E-7B2C-7CC1-8C23-5B8F7B3AA001",
        "01890f3e-7b2c-6cc1-8c23-5b8f7b3aa001",
        "01890f3e-7b2c-7cc1-7c23-5b8f7b3aa001",
        "not-a-uuid",
    ] { #expect(ProtocolID(rawValue: invalid) == nil) }
    #expect(Cursor(rawValue: -1) == nil)
    #expect(throws: Error.self) {
        _ = try JSONDecoder().decode(Cursor.self, from: Data("9223372036854775808".utf8))
    }
}

@Test func unknownEphemeralApplicationEventFailsInProductionFrameDecoder() throws {
    var object = try #require(
        JSONSerialization.jsonObject(with: fixture("ephemeral-drafts.json")) as? [[String: Any]])[0]
    object["event_type"] = "assistant.future_draft"
    #expect(throws: ProtocolModelError.unknownEventType) {
        _ = try ServerFrame.decode(JSONSerialization.data(withJSONObject: object))
    }
}
