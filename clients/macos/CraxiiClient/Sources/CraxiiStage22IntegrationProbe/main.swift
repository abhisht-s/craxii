import Foundation
import Darwin
import CraxiiProtocol
import CraxiiClientCore
import CraxiiAppleAdapters

@main
struct CraxiiStage22IntegrationProbe {
    static func main() async {
        guard ProcessInfo.processInfo.environment["CRAXII_STAGE22_INTEGRATION"] == "1" else {
            emit("INTEGRATION_DISABLED")
            return
        }
        do {
            try await run()
            emit("STAGE22_NATIVE_INTEGRATION_PASSED")
        } catch let error as ClientError {
            emit("STAGE22_NATIVE_INTEGRATION_FAILED:\(error.description)")
            exit(1)
        } catch {
            emit("STAGE22_NATIVE_INTEGRATION_FAILED:unexpected")
            exit(1)
        }
    }

    private static func run() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard let endpointText = environment["CRAXII_STAGE22_ENDPOINT"],
              let credentialText = environment["CRAXII_STAGE22_TOKEN"],
              let profileText = environment["CRAXII_STAGE22_PROFILE_ID"],
              let statePath = environment["CRAXII_STAGE22_STATE_DIR"],
              let profileID = ProtocolID(rawValue: profileText) else {
            throw ClientError.configurationMismatch
        }
        let endpoint = try EndpointPolicy.validate(endpointText, allowDebugLocalhostHTTP: true)
        let profile = BackendProfile(profileID: profileID, endpoint: endpoint.absoluteString)
        let credential = try BearerToken(validating: credentialText)
        let keychain = KeychainDeviceCredentialStore()
        let state = try AtomicFileStateStore(
            directory: URL(fileURLWithPath: statePath, isDirectory: true))
        try? await keychain.delete(profileID: profileID)
        try await keychain.add(token: credential, profileID: profileID)
        do {
            let session = ClientSession(
                profile: profile, allowDebugLocalhostHTTP: true,
                credentialStore: keychain, localStore: state,
                http: URLSessionHTTPExecutor(), streams: URLSessionEventStreamOpener())
            await session.start()
            try await wait(session) { $0.connectionState == .live }
            emit("BOOTSTRAP_LIVE")

            let first = try await session.prepareMessage(text: "Stage 22 first work")
            try await assertPreparedMessage(first, text: "Stage 22 first work", session: session)
            emit("FIRST_PREPARED")
            let firstReceipt = try await session.sendPreparedMessage(first)
            try await wait(session) { snapshot in
                snapshot.drafts.contains { $0.workID == firstReceipt.workID }
            }

            let second = try await session.prepareMessage(text: "Stage 22 queued second work")
            let secondReceipt = try await session.sendPreparedMessage(second)
            try await wait(session) { snapshot in
                snapshot.drafts.contains { $0.workID == firstReceipt.workID }
                    && snapshot.projection.works.first(where: {
                        $0.workID == secondReceipt.workID
                    })?.state == .queued
            }
            emit("FIRST_DRAFT_SECOND_QUEUED")

            try await wait(session) { snapshot in
                let firstCommitted = snapshot.projection.messages.contains {
                    $0.role == .assistant && $0.workID == firstReceipt.workID
                        && $0.content.first?.text == "stage22 first authoritative answer"
                }
                let secondActive = snapshot.drafts.contains { $0.workID == secondReceipt.workID }
                return firstCommitted && secondActive
            }
            emit("FIFO_SECOND_ACTIVE")

            await session.suspendTransport()
            let disconnected = await session.currentSnapshot()
            guard disconnected.drafts.isEmpty,
                  disconnected.projection.works.first(where: {
                      $0.workID == secondReceipt.workID
                  }).map({ !$0.state.isTerminal }) == true else {
                throw ClientError.projectionInvariant
            }
            emit("DISCONNECT_CLEARED_DRAFT")
            try await Task.sleep(for: .milliseconds(50))
            await session.resumeTransport()
            try await wait(session) { snapshot in
                snapshot.connectionState == .live
                    && snapshot.projection.messages.contains {
                        $0.role == .assistant && $0.workID == secondReceipt.workID
                            && $0.content.first?.text == "stage22 second authoritative answer"
                    }
            }
            let reconnected = await session.currentSnapshot()
            guard reconnected.drafts.isEmpty,
                  Set(reconnected.projection.messages.map(\.messageID)).count
                    == reconnected.projection.messages.count else {
                throw ClientError.projectionInvariant
            }
            emit("RECONNECT_REPLAY_NO_DUPLICATES")

            let third = try await session.prepareMessage(text: "Stage 22 cancellable work")
            let thirdReceipt = try await session.sendPreparedMessage(third)
            try await wait(session) { snapshot in
                snapshot.drafts.contains { $0.workID == thirdReceipt.workID }
            }
            let cancellation = try await session.prepareCancellation(workID: thirdReceipt.workID)
            let cancellationPending = await session.currentSnapshot().pendingCommands.first {
                $0.commandID == cancellation.clientCommandID
            }
            guard cancellationPending?.kind == .cancellation,
                  cancellationPending?.workID == thirdReceipt.workID,
                  cancellationPending?.deliveryState == .notSent else {
                throw ClientError.projectionInvariant
            }
            emit("CANCELLATION_PREPARED")
            _ = try await session.sendPreparedCancellation(cancellation)
            try await wait(session) { snapshot in
                snapshot.projection.works.first(where: {
                    $0.workID == thirdReceipt.workID
                })?.state.isTerminal == true
            }
            let terminal = await session.currentSnapshot()
            guard let cancelledWork = terminal.projection.works.first(where: {
                $0.workID == thirdReceipt.workID
            }), cancelledWork.state == .cancelled || cancelledWork.state == .interrupted,
                  terminal.drafts.allSatisfy({ $0.workID != thirdReceipt.workID }) else {
                throw ClientError.projectionInvariant
            }
            emit("CANCELLATION_DURABLE_TRUTH")

            let liveProjection = terminal.projection
            await session.retryConnection()
            try await wait(session) { $0.connectionState == .live }
            let bootstrapped = await session.currentSnapshot().projection
            guard bootstrapped.craxii == liveProjection.craxii,
                  bootstrapped.primaryConversation == liveProjection.primaryConversation,
                  sameOrderedMessages(bootstrapped.messages, liveProjection.messages),
                  bootstrapped.works == liveProjection.works,
                  bootstrapped.unresolvedOutcomes == liveProjection.unresolvedOutcomes else {
                throw ClientError.projectionInvariant
            }
            emit("FINAL_BOOTSTRAP_CONVERGED")
            await session.shutdown()
            try await keychain.delete(profileID: profileID)
        } catch {
            try? await keychain.delete(profileID: profileID)
            throw error
        }
    }

    private static func assertPreparedMessage(
        _ prepared: PreparedMessageCommand, text: String, session: ClientSession
    ) async throws {
        let pending = await session.currentSnapshot().pendingCommands.first {
            $0.commandID == prepared.clientMessageID
        }
        guard pending?.kind == .message,
              pending?.clientMessageID == prepared.clientMessageID,
              pending?.visibleMessageText == text,
              pending?.deliveryState == .notSent else {
            throw ClientError.projectionInvariant
        }
    }

    private static func wait(
        _ session: ClientSession, predicate: (ClientSnapshot) -> Bool
    ) async throws {
        for _ in 0..<4_000 {
            let snapshot = await session.currentSnapshot()
            if predicate(snapshot) { return }
            if snapshot.connectionState == .fatalProtocolError
                || snapshot.connectionState == .authenticationFailed {
                throw snapshot.lastError ?? ClientError.projectionInvariant
            }
            try await Task.sleep(for: .milliseconds(5))
        }
        throw ClientError.timeout
    }

    private static func sameOrderedMessages(
        _ left: [ProjectedMessage], _ right: [ProjectedMessage]
    ) -> Bool {
        guard left.count == right.count else { return false }
        return zip(left, right).allSatisfy { lhs, rhs in
            lhs.messageID == rhs.messageID
                && lhs.conversationID == rhs.conversationID
                && lhs.role == rhs.role
                && lhs.content == rhs.content
                && lhs.clientMessageID == rhs.clientMessageID
                && lhs.workID == rhs.workID
                && lhs.committedAt == rhs.committedAt
        }
    }

    private static func emit(_ line: String) {
        FileHandle.standardOutput.write(Data("\(line)\n".utf8))
    }
}
