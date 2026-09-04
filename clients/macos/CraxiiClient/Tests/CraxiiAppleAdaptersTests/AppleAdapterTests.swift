import Foundation
import Darwin
import Security
import Testing
@testable import CraxiiProtocol
@testable import CraxiiClientCore
@testable import CraxiiAppleAdapters

private let validToken = String(repeating: "b", count: 64)
private func uniqueProfileID() -> ProtocolID {
    let suffix = UInt64.random(in: 0 ... 0xffffffffffff)
    return ProtocolID(rawValue: String(format: "01890f6c-7b3a-7000-8000-%012llx", suffix))!
}

@Test func keychainMissingAddUpdateReadDeleteAndMalformedAreExact() async throws {
    let store = KeychainDeviceCredentialStore()
    let profile = uniqueProfileID()
    try? await store.delete(profileID: profile)
    await #expect(throws: ClientError.credentialRequired) { _ = try await store.read(profileID: profile) }
    let first = try BearerToken(validating: validToken)
    try await store.add(token: first, profileID: profile)
    #expect(try await store.read(profileID: profile) == first)
    let second = try BearerToken(validating: String(repeating: "c", count: 64))
    try await store.update(token: second, profileID: profile)
    #expect(try await store.read(profileID: profile) == second)
    try await store.delete(profileID: profile)
    await #expect(throws: ClientError.credentialRequired) { _ = try await store.read(profileID: profile) }

    let malformed: [String: Any] = [
        kSecClass as String: kSecClassGenericPassword,
        kSecAttrService as String: KeychainDeviceCredentialStore.service,
        kSecAttrAccount as String: profile.rawValue,
        kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        kSecAttrSynchronizable as String: false,
        kSecValueData as String: Data("malformed".utf8),
    ]
    #expect(SecItemAdd(malformed as CFDictionary, nil) == errSecSuccess)
    await #expect(throws: ClientError.credentialMalformed) { _ = try await store.read(profileID: profile) }
    var query = malformed
    query.removeValue(forKey: kSecValueData as String)
    query[kSecReturnData as String] = true
    var item: CFTypeRef?
    #expect(SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess)
    try await store.delete(profileID: profile)
}

@Test func atomicLocalStoreUsesOwnerOnlyPermissionsAndQuarantinesCorruption() async throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent(
        "craxii-stage21-store-\(UUID().uuidString)", isDirectory: true)
    defer { try? FileManager.default.removeItem(at: root) }
    let store = try AtomicFileStateStore(directory: root)
    let state = DisposableClientState()
    try await store.save(state)
    #expect(try await store.load() == state)
    let file = store.stateFileURL
    let attributes = try FileManager.default.attributesOfItem(atPath: file.path)
    #expect((attributes[.posixPermissions] as? NSNumber)?.intValue == 0o600)
    try Data("not-json".utf8).write(to: file, options: .atomic)
    await #expect(throws: ClientError.cacheCorrupt) { _ = try await store.load() }
    let quarantine = try FileManager.default.contentsOfDirectory(atPath: root.path)
    #expect(quarantine.contains(where: { $0.hasPrefix("client-state-v1.corrupt-") }))
}

private final class URLStub: URLProtocol, @unchecked Sendable {
    nonisolated(unsafe) static var handler: ((URLRequest) throws -> (HTTPURLResponse, Data))?
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        do {
            let result = try Self.handler!(request)
            client?.urlProtocol(self, didReceive: result.0, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: result.1)
            client?.urlProtocolDidFinishLoading(self)
        } catch { client?.urlProtocol(self, didFailWithError: error) }
    }
    override func stopLoading() {}
}

@Test func nativeHTTPAdapterBuildsTransientAuthAndHonorsResponseBound() async throws {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.protocolClasses = [URLStub.self]
    let executor = URLSessionHTTPExecutor(configuration: configuration)
    let token = try BearerToken(validating: validToken)
    URLStub.handler = { request in
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer \(validToken)")
        #expect(request.value(forHTTPHeaderField: "Idempotency-Key") == "stable-id")
        let response = HTTPURLResponse(
            url: request.url!, statusCode: 202, httpVersion: "HTTP/1.1",
            headerFields: [
                "Content-Type": "application/json",
                "x-request-id": "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d",
            ])!
        return (response, Data("{}".utf8))
    }
    let accepted = try await executor.execute(HTTPRequest(
        url: URL(string: "https://example.test/v1/x")!, method: "POST",
        authorization: token, idempotencyKey: "stable-id", body: Data("{}".utf8),
        timeout: 15, maximumResponseBytes: 1_024))
    #expect(accepted.statusCode == 202)
    #expect(accepted.requestID?.rawValue == "01890f6c-7b3a-7cc0-98f1-2e6f7a8b9c0d")

    URLStub.handler = { request in
        (HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!,
         Data(repeating: 1, count: 65))
    }
    await #expect(throws: ClientError.malformedPayload) {
        _ = try await executor.execute(HTTPRequest(
            url: URL(string: "https://example.test/v1/bootstrap")!, method: "GET",
            authorization: token, timeout: 35, maximumResponseBytes: 64))
    }
    URLStub.handler = nil
}

@Test func disposableStateEncodingContainsNoCredentialOrAuthorizationResidue() throws {
    let encoded = try JSONEncoder().encode(DisposableClientState())
    let text = String(decoding: encoded, as: UTF8.self)
    #expect(!text.contains(validToken))
    #expect(!text.localizedCaseInsensitiveContains("authorization"))
    #expect(!text.contains("Bearer"))
}
