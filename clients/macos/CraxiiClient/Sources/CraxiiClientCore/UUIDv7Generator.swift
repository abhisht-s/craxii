import Foundation
import CraxiiProtocol

public protocol UUIDv7Clock: Sendable { func millisecondsSince1970() -> UInt64 }
public protocol UUIDv7RandomSource: Sendable { func bytes(count: Int) throws -> [UInt8] }

public struct SystemUUIDv7Clock: UUIDv7Clock {
    public init() {}
    public func millisecondsSince1970() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1_000)
    }
}

public struct SystemUUIDv7RandomSource: UUIDv7RandomSource {
    public init() {}
    public func bytes(count: Int) throws -> [UInt8] {
        var generator = SystemRandomNumberGenerator()
        return (0..<count).map { _ in UInt8.random(in: .min ... .max, using: &generator) }
    }
}

public actor UUIDv7Generator: UUIDv7Generating {
    private let clock: any UUIDv7Clock
    private let random: any UUIDv7RandomSource

    public init(clock: any UUIDv7Clock = SystemUUIDv7Clock(), random: any UUIDv7RandomSource = SystemUUIDv7RandomSource()) {
        self.clock = clock
        self.random = random
    }

    public func next() throws -> ProtocolID {
        let milliseconds = clock.millisecondsSince1970()
        guard milliseconds < (1 << 48) else { throw ClientError.configurationMismatch }
        let randomBytes = try random.bytes(count: 10)
        guard randomBytes.count == 10 else { throw ClientError.configurationMismatch }
        var bytes = [UInt8](repeating: 0, count: 16)
        for index in 0..<6 { bytes[index] = UInt8((milliseconds >> UInt64((5 - index) * 8)) & 0xff) }
        bytes[6] = 0x70 | (randomBytes[0] & 0x0f)
        bytes[7] = randomBytes[1]
        bytes[8] = 0x80 | (randomBytes[2] & 0x3f)
        for index in 9..<16 { bytes[index] = randomBytes[index - 6] }
        let value = String(format:
            "%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15])
        guard let id = ProtocolID(rawValue: value) else { throw ClientError.configurationMismatch }
        return id
    }
}
