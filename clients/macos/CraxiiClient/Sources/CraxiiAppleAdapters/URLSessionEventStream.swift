import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import CraxiiClientCore

private final class NoWebSocketRedirectDelegate: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(
        _ session: URLSession, task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}

public final class URLSessionEventStreamOpener: EventStreamOpening, @unchecked Sendable {
    public init() {}

    public func open(url: URL, authorization: BearerToken) async throws -> any EventStreamConnection {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpShouldSetCookies = false
        configuration.httpCookieAcceptPolicy = .never
        configuration.httpCookieStorage = nil
        configuration.urlCache = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        let session = URLSession(
            configuration: configuration, delegate: NoWebSocketRedirectDelegate(),
            delegateQueue: nil)
        var request = URLRequest(url: url)
        request.timeoutInterval = 35
        request.httpShouldHandleCookies = false
        authorization.withValue { request.setValue("Bearer \($0)", forHTTPHeaderField: "Authorization") }
        let task = session.webSocketTask(with: request)
        task.maximumMessageSize = 270_336
        task.resume()
        return URLSessionEventStreamConnection(session: session, task: task)
    }
}

public actor URLSessionEventStreamConnection: EventStreamConnection {
    private let session: URLSession
    private let task: URLSessionWebSocketTask
    private var closed = false

    init(session: URLSession, task: URLSessionWebSocketTask) {
        self.session = session
        self.task = task
    }

    public func receive() async throws -> EventStreamMessage {
        guard !closed else { throw ClientError.networkOffline }
        do {
            switch try await task.receive() {
            case let .string(text):
                let data = Data(text.utf8)
                guard data.count <= 270_336 else { throw ClientError.malformedPayload }
                return .text(data)
            case .data:
                return .binary
            @unknown default:
                throw ClientError.malformedPayload
            }
        } catch let error as URLError where error.code == .userAuthenticationRequired {
            throw ClientError.authentication
        } catch let error as URLError where error.code == .timedOut {
            throw ClientError.timeout
        } catch {
            if let response = task.response as? HTTPURLResponse {
                if response.statusCode == 401 { throw ClientError.authentication }
                if [502, 503, 504].contains(response.statusCode) {
                    throw ClientError.serverUnavailable
                }
            }
            throw ClientError.serverUnavailable
        }
    }

    public func ping() async throws {
        guard !closed else { throw ClientError.networkOffline }
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            task.sendPing { error in
                if let error { continuation.resume(throwing: error) }
                else { continuation.resume() }
            }
        }
    }

    public func close() {
        guard !closed else { return }
        closed = true
        task.cancel(with: .goingAway, reason: nil)
        session.invalidateAndCancel()
    }
}
