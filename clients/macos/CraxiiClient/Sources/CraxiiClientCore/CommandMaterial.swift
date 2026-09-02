import Foundation
import CryptoKit

public enum CommandMaterial {
    public static func hash(method: String, path: String, idempotencyKey: String, body: Data) -> String {
        var material = Data()
        for field in [Data(method.utf8), Data(path.utf8), Data(idempotencyKey.utf8), body] {
            var size = UInt64(field.count).bigEndian
            withUnsafeBytes(of: &size) { material.append(contentsOf: $0) }
            material.append(field)
        }
        return SHA256.hash(data: material).map { String(format: "%02x", $0) }.joined()
    }
}
