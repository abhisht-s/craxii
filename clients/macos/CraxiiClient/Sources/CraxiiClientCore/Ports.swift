import Foundation
import CraxiiProtocol

public protocol CredentialStoring: Sendable {
    func add(token: BearerToken, profileID: ProtocolID) async throws
    func update(token: BearerToken, profileID: ProtocolID) async throws
    func read(profileID: ProtocolID) async throws -> BearerToken
    func delete(profileID: ProtocolID) async throws
}

public protocol LocalStateStoring: Sendable {
    func load() async throws -> DisposableClientState
    func save(_ state: DisposableClientState) async throws
    func reset() async throws
}

public struct HTTPRequest: Sendable {
    public let url: URL
    public let method: String
    public let authorization: BearerToken
    public let idempotencyKey: String?
    public let body: Data?
    public let timeout: TimeInterval
    public let maximumResponseBytes: Int

    public init(
        url: URL, method: String, authorization: BearerToken, idempotencyKey: String? = nil,
        body: Data? = nil, timeout: TimeInterval, maximumResponseBytes: Int
    ) {
        self.url = url
        self.method = method
        self.authorization = authorization
        self.idempotencyKey = idempotencyKey
        self.body = body
        self.timeout = timeout
        self.maximumResponseBytes = maximumResponseBytes
    }
}

public struct HTTPResponse: Sendable {
    public let statusCode: Int
    public let body: Data
    public init(statusCode: Int, body: Data) { self.statusCode = statusCode; self.body = body }
}

public protocol HTTPExecuting: Sendable {
    func execute(_ request: HTTPRequest) async throws -> HTTPResponse
}

public enum EventStreamMessage: Sendable { case text(Data), binary }

public protocol EventStreamConnection: Sendable {
    func receive() async throws -> EventStreamMessage
    func ping() async throws
    func close() async
}

public protocol EventStreamOpening: Sendable {
    func open(url: URL, authorization: BearerToken) async throws -> any EventStreamConnection
}

public protocol ClientSleeping: Sendable {
    func sleep(for duration: Duration) async throws
}

public struct ContinuousClientSleeper: ClientSleeping {
    public init() {}
    public func sleep(for duration: Duration) async throws { try await Task.sleep(for: duration) }
}

public protocol NetworkStatusProviding: Sendable { func isOnline() async -> Bool }
public struct AlwaysOnlineNetworkStatus: NetworkStatusProviding {
    public init() {}
    public func isOnline() async -> Bool { true }
}

public protocol UUIDv7Generating: Sendable { func next() async throws -> ProtocolID }
