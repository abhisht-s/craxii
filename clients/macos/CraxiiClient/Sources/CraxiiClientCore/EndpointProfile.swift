import Foundation
import CraxiiProtocol

public struct BackendProfile: Codable, Equatable, Sendable {
    public let profileID: ProtocolID
    public let endpoint: String
    public let credentialGeneration: UInt64

    public init(profileID: ProtocolID, endpoint: String, credentialGeneration: UInt64 = 0) {
        self.profileID = profileID
        self.endpoint = endpoint
        self.credentialGeneration = credentialGeneration
    }

    public func replacingCredential() -> Self {
        Self(profileID: profileID, endpoint: endpoint, credentialGeneration: credentialGeneration + 1)
    }
}

public enum EndpointPolicy {
    public static func validate(_ input: String, allowDebugLocalhostHTTP: Bool) throws -> URL {
        guard input == input.trimmingCharacters(in: .whitespacesAndNewlines),
              var components = URLComponents(string: input),
              components.user == nil, components.password == nil,
              components.query == nil, components.fragment == nil,
              let scheme = components.scheme?.lowercased(),
              let host = components.host?.lowercased(), !host.isEmpty,
              components.percentEncodedPath == "" || components.percentEncodedPath == "/",
              !components.percentEncodedPath.contains("%")
        else { throw ClientError.configurationMismatch }

        let local = host == "localhost" || host == "127.0.0.1" || host == "::1"
        guard scheme == "https" || (scheme == "http" && local && allowDebugLocalhostHTTP) else {
            throw ClientError.configurationMismatch
        }
        components.scheme = scheme
        components.host = host
        components.path = "/"
        guard let normalized = components.url else { throw ClientError.configurationMismatch }
        return normalized
    }

    public static func webSocketURL(baseURL: URL, cursor: Cursor) throws -> URL {
        guard var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false) else {
            throw ClientError.configurationMismatch
        }
        components.scheme = baseURL.scheme == "https" ? "wss" : "ws"
        components.path = "/v1/events"
        components.queryItems = [URLQueryItem(name: "after", value: String(cursor.rawValue))]
        guard let url = components.url else { throw ClientError.configurationMismatch }
        return url
    }

    public static func endpoint(baseURL: URL, exactPath: String) throws -> URL {
        guard exactPath.hasPrefix("/"), !exactPath.contains("?"), !exactPath.contains("#"),
              var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false)
        else { throw ClientError.configurationMismatch }
        components.path = exactPath
        guard let url = components.url else { throw ClientError.configurationMismatch }
        return url
    }
}
