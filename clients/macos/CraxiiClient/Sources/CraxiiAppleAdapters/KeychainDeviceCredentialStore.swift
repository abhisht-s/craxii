import Foundation
import Security
import CraxiiProtocol
import CraxiiClientCore

public actor KeychainDeviceCredentialStore: CredentialStoring {
    public static let service = "com.craxii.device-token.v1"

    public init() {}

    public func add(token: BearerToken, profileID: ProtocolID) throws {
        let data = token.withValue { Data($0.utf8) }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: profileID.rawValue,
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
            kSecAttrSynchronizable as String: false,
            kSecValueData as String: data,
        ]
        let status = SecItemAdd(query as CFDictionary, nil)
        guard status == errSecSuccess else { throw ClientError.keychainFailure(status) }
    }

    public func update(token: BearerToken, profileID: ProtocolID) throws {
        let query = baseQuery(profileID: profileID)
        let attributes: [String: Any] = [
            kSecValueData as String: token.withValue { Data($0.utf8) },
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        let status = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if status == errSecItemNotFound { throw ClientError.credentialRequired }
        guard status == errSecSuccess else { throw ClientError.keychainFailure(status) }
    }

    public func read(profileID: ProtocolID) throws -> BearerToken {
        var query = baseQuery(profileID: profileID)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecItemNotFound { throw ClientError.credentialRequired }
        guard status == errSecSuccess, let data = item as? Data,
              let text = String(data: data, encoding: .utf8) else {
            if status == errSecSuccess { throw ClientError.credentialMalformed }
            throw ClientError.keychainFailure(status)
        }
        return try BearerToken(validating: text)
    }

    public func delete(profileID: ProtocolID) throws {
        let status = SecItemDelete(baseQuery(profileID: profileID) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw ClientError.keychainFailure(status)
        }
    }

    private func baseQuery(profileID: ProtocolID) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: Self.service,
            kSecAttrAccount as String: profileID.rawValue,
            kSecAttrSynchronizable as String: false,
        ]
    }
}
