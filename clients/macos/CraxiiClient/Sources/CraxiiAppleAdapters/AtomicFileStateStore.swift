import Foundation
import Darwin
import CraxiiClientCore

public actor AtomicFileStateStore: LocalStateStoring {
    private let directory: URL
    private let file: URL
    private let manager = FileManager.default

    public init(directory: URL? = nil) throws {
        if let directory {
            self.directory = directory
        } else {
            let applicationSupport = try FileManager.default.url(
                for: .applicationSupportDirectory, in: .userDomainMask,
                appropriateFor: nil, create: true)
            self.directory = applicationSupport.appendingPathComponent(
                "com.craxii.client.macos", isDirectory: true)
        }
        file = self.directory.appendingPathComponent("client-state-v1.json", isDirectory: false)
    }

    public func load() throws -> DisposableClientState {
        guard manager.fileExists(atPath: file.path) else { return DisposableClientState() }
        do {
            return try JSONDecoder().decode(DisposableClientState.self, from: Data(contentsOf: file))
        } catch {
            try quarantineCorruptFile()
            throw ClientError.cacheCorrupt
        }
    }

    public func save(_ state: DisposableClientState) throws {
        try ensureDirectory()
        let data = try JSONEncoder().encode(state)
        try data.write(to: file, options: [.atomic])
        guard chmod(file.path, S_IRUSR | S_IWUSR) == 0 else { throw ClientError.cacheCorrupt }
    }

    public func reset() throws {
        guard manager.fileExists(atPath: file.path) else { return }
        try manager.removeItem(at: file)
    }

    public nonisolated var stateFileURL: URL { file }

    private func ensureDirectory() throws {
        if !manager.fileExists(atPath: directory.path) {
            try manager.createDirectory(
                at: directory, withIntermediateDirectories: true,
                attributes: [.posixPermissions: NSNumber(value: Int16(0o700))])
        }
        guard chmod(directory.path, S_IRWXU) == 0 else { throw ClientError.cacheCorrupt }
    }

    private func quarantineCorruptFile() throws {
        let quarantine = directory.appendingPathComponent(
            "client-state-v1.corrupt-\(UUID().uuidString.lowercased()).json")
        try manager.moveItem(at: file, to: quarantine)
        _ = chmod(quarantine.path, S_IRUSR | S_IWUSR)
    }
}
