import Foundation
import CraxiiProtocol
import CraxiiClientCore

protocol ConversationSession: Sendable {
    func setSnapshotHandler(_ handler: (@Sendable (ClientSnapshot) -> Void)?) async
    func currentSnapshot() async -> ClientSnapshot
    func start() async
    func retryConnection() async
    func installCredential(_ text: String) async throws
    func deleteCredential() async throws
    func resetDisposableState() async throws
    func changeEndpoint(to input: String) async throws
    func prepareMessage(text: String) async throws -> PreparedMessageCommand
    func sendPreparedMessage(_ prepared: PreparedMessageCommand) async throws -> MessageReceipt
    func prepareCancellation(workID: WorkID) async throws -> PreparedCancellationCommand
    func sendPreparedCancellation(
        _ prepared: PreparedCancellationCommand
    ) async throws -> CancellationReceipt
    func suspendTransport() async
    func resumeTransport() async
    func shutdown() async
}

extension ClientSession: ConversationSession {}
