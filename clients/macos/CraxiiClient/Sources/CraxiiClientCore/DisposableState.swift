import Foundation
import CraxiiProtocol

public enum CommandKind: String, Codable, Sendable { case message, cancellation }
public enum OutboxDisposition: String, Codable, Sendable { case sendable, reconciliationOnly }

public struct PendingCommand: Codable, Equatable, Sendable, Identifiable {
    public let kind: CommandKind
    public let commandID: ProtocolID
    public let path: String
    public let body: Data
    public let idempotencyKey: String
    public let materialHash: String
    public let profileID: ProtocolID
    public let craxiiID: ProtocolID?
    public let credentialGeneration: UInt64
    public var attempts: Int
    public var disposition: OutboxDisposition
    public var id: String { commandID.rawValue }

    public init(
        kind: CommandKind, commandID: ProtocolID, path: String, body: Data,
        idempotencyKey: String, materialHash: String, profileID: ProtocolID,
        craxiiID: ProtocolID?, credentialGeneration: UInt64, attempts: Int = 0,
        disposition: OutboxDisposition = .sendable
    ) {
        self.kind = kind; self.commandID = commandID; self.path = path; self.body = body
        self.idempotencyKey = idempotencyKey; self.materialHash = materialHash
        self.profileID = profileID; self.craxiiID = craxiiID
        self.credentialGeneration = credentialGeneration; self.attempts = attempts
        self.disposition = disposition
    }
}

public struct DisposableClientState: Codable, Equatable, Sendable {
    public static let schemaVersion = 1
    public var schema: Int
    public var profile: BackendProfile?
    public var boundCraxiiID: ProtocolID?
    public var protocolVersion: Int
    public var lastAppliedCursor: Cursor
    public var outbox: [PendingCommand]

    public init(
        schema: Int = schemaVersion, profile: BackendProfile? = nil,
        boundCraxiiID: ProtocolID? = nil, protocolVersion: Int = ProtocolConstants.version,
        lastAppliedCursor: Cursor = .start, outbox: [PendingCommand] = []
    ) {
        self.schema = schema; self.profile = profile; self.boundCraxiiID = boundCraxiiID
        self.protocolVersion = protocolVersion; self.lastAppliedCursor = lastAppliedCursor
        self.outbox = outbox
    }

    public func validated(maximumOutboxEntries: Int = 128) throws -> Self {
        guard schema == Self.schemaVersion, protocolVersion == ProtocolConstants.version,
              outbox.count <= maximumOutboxEntries else { throw ClientError.cacheCorrupt }
        for entry in outbox {
            guard entry.idempotencyKey == entry.commandID.rawValue,
                  entry.attempts >= 0, entry.attempts <= ClientSession.maximumCommandAttempts,
                  entry.body.count <= 512 * 1_024,
                  CommandMaterial.hash(
                    method: "POST", path: entry.path, idempotencyKey: entry.idempotencyKey,
                    body: entry.body) == entry.materialHash
            else { throw ClientError.outboxCorrupt }
            try validateShape(entry)
        }
        return self
    }

    private func validateShape(_ entry: PendingCommand) throws {
        let components = entry.path.split(separator: "/", omittingEmptySubsequences: false)
        let decoder = JSONDecoder()
        switch entry.kind {
        case .message:
            guard components.count == 5, components[0].isEmpty,
                  components[1] == "v1", components[2] == "conversations",
                  ProtocolID(rawValue: String(components[3])) != nil,
                  components[4] == "messages",
                  let request = try? decoder.decode(MessageRequest.self, from: entry.body),
                  request.clientMessageID == entry.commandID,
                  !request.content.isEmpty else {
                throw ClientError.outboxCorrupt
            }
        case .cancellation:
            guard components.count == 5, components[0].isEmpty,
                  components[1] == "v1", components[2] == "work-items",
                  ProtocolID(rawValue: String(components[3])) != nil,
                  components[4] == "cancel",
                  let request = try? decoder.decode(CancellationRequest.self, from: entry.body),
                  request.clientCommandID == entry.commandID else {
                throw ClientError.outboxCorrupt
            }
        }
    }
}
