import Foundation
import Darwin
import CraxiiProtocol
import CraxiiClientCore
import CraxiiAppleAdapters

@main
struct CraxiiIntegrationProbe {
    static func main() async {
        guard ProcessInfo.processInfo.environment["CRAXII_STAGE21_INTEGRATION"] == "1" else {
            emit("INTEGRATION_DISABLED")
            return
        }
        do {
            try await run()
            emit("STAGE21_NATIVE_INTEGRATION_PASSED")
        } catch let error as ClientError {
            emit("STAGE21_NATIVE_INTEGRATION_FAILED:\(error.description)")
            exit(1)
        } catch {
            emit("STAGE21_NATIVE_INTEGRATION_FAILED:unexpected")
            exit(1)
        }
    }

    private static func run() async throws {
        let environment = ProcessInfo.processInfo.environment
        guard let endpointText = environment["CRAXII_STAGE21_ENDPOINT"],
              let credentialText = environment["CRAXII_STAGE21_TOKEN"],
              let profileText = environment["CRAXII_STAGE21_PROFILE_ID"],
              let statePath = environment["CRAXII_STAGE21_STATE_DIR"],
              let profileID = ProtocolID(rawValue: profileText) else {
            throw ClientError.configurationMismatch
        }
        let endpoint = try EndpointPolicy.validate(endpointText, allowDebugLocalhostHTTP: true)
        let profile = BackendProfile(profileID: profileID, endpoint: endpoint.absoluteString)
        let credential = try BearerToken(validating: credentialText)
        let keychain = KeychainDeviceCredentialStore()
        let state = try AtomicFileStateStore(directory: URL(fileURLWithPath: statePath, isDirectory: true))
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

            _ = try await session.submitMessage(text: "Stage 21 native transport draft and commit")
            try await wait(session) { !$0.drafts.isEmpty }
            emit("DRAFT_OBSERVED")
            try await wait(session) { snapshot in
                snapshot.drafts.isEmpty && snapshot.projection.messages.contains {
                    $0.role == .assistant && $0.content.first?.text == "stage21 authoritative answer"
                }
            }
            let afterCommit = await session.currentSnapshot()
            let committedMessageCount = afterCommit.projection.messages.count
            emit("COMMIT_RECONCILED")

            await session.suspendTransport()
            guard await session.currentSnapshot().drafts.isEmpty else {
                throw ClientError.projectionInvariant
            }
            await session.resumeTransport()
            try await wait(session) { $0.connectionState == .live }
            let reconnected = await session.currentSnapshot()
            guard reconnected.drafts.isEmpty,
                  reconnected.projection.messages.count == committedMessageCount else {
                throw ClientError.projectionInvariant
            }
            emit("RECONNECT_NO_DRAFT")

            let delayed = try await session.submitMessage(text: "Stage 21 cancellable work")
            try await wait(session) { snapshot in
                snapshot.drafts.contains { $0.workID == delayed.workID }
            }
            _ = try await session.cancel(workID: delayed.workID)
            do {
                try await wait(session) { snapshot in
                    snapshot.projection.works.first(where: { $0.workID == delayed.workID })?.state == .interrupted
                }
            } catch {
                let diagnostic = await session.currentSnapshot()
                emit("CANCELLATION_WAIT_STATE:\(diagnostic.connectionState.rawValue):target=\(delayed.workID.rawValue):\(diagnostic.projection.works.map { "\($0.workID.rawValue)=\($0.state.rawValue)" }.joined(separator: ","))")
                throw error
            }
            let beforeBootstrap = await session.currentSnapshot().projection
            emit("CANCELLATION_AUTHORITATIVE")

            await session.retryConnection()
            try await wait(session) { $0.connectionState == .live }
            let fresh = await session.currentSnapshot().projection
            guard fresh.craxii == beforeBootstrap.craxii,
                  fresh.primaryConversation == beforeBootstrap.primaryConversation,
                  sameOrderedMessages(fresh.messages, beforeBootstrap.messages),
                  fresh.works == beforeBootstrap.works,
                  fresh.unresolvedOutcomes == beforeBootstrap.unresolvedOutcomes else {
                throw ClientError.projectionInvariant
            }
            emit("BOOTSTRAP_CONVERGED")
            await session.shutdown()
            try await keychain.delete(profileID: profileID)
        } catch {
            try? await keychain.delete(profileID: profileID)
            throw error
        }
    }

    private static func wait(
        _ session: ClientSession, predicate: (ClientSnapshot) -> Bool
    ) async throws {
        for _ in 0..<2_000 {
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

    private static func emit(_ line: String) {
        FileHandle.standardOutput.write(Data("\(line)\n".utf8))
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
}
