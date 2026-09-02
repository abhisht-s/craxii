import Foundation

public enum ClientError: Error, Equatable, Sendable, CustomStringConvertible {
    case credentialRequired
    case credentialMalformed
    case keychainFailure(Int32)
    case networkOffline
    case timeout
    case authentication
    case serverUnavailable
    case serverNotReady
    case incompatibleProtocol
    case malformedPayload
    case projectionInvariant
    case commandRejected(String)
    case cancellationTransportFailure
    case cacheCorrupt
    case outboxCorrupt
    case configurationMismatch

    public var description: String {
        switch self {
        case .credentialRequired: "credentialRequired"
        case .credentialMalformed: "credentialMalformed"
        case .keychainFailure: "keychainFailure"
        case .networkOffline: "networkOffline"
        case .timeout: "timeout"
        case .authentication: "authentication"
        case .serverUnavailable: "serverUnavailable"
        case .serverNotReady: "serverNotReady"
        case .incompatibleProtocol: "incompatibleProtocol"
        case .malformedPayload: "malformedPayload"
        case .projectionInvariant: "projectionInvariant"
        case .commandRejected: "commandRejected"
        case .cancellationTransportFailure: "cancellationTransportFailure"
        case .cacheCorrupt: "cacheCorrupt"
        case .outboxCorrupt: "outboxCorrupt"
        case .configurationMismatch: "configurationMismatch"
        }
    }
}

public enum CredentialStatus: String, Sendable { case required, installed, malformed }

public struct BearerToken: Equatable, Sendable {
    private let value: String

    public init(validating value: String) throws {
        guard value.utf8.count == 64,
              value.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) })
        else { throw ClientError.credentialMalformed }
        self.value = value
    }

    public func withValue<T>(_ operation: (String) throws -> T) rethrows -> T {
        try operation(value)
    }
}

extension BearerToken: CustomStringConvertible, CustomDebugStringConvertible {
    public var description: String { "<redacted>" }
    public var debugDescription: String { "BearerToken(<redacted>)" }
}
