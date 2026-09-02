import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif
import CraxiiClientCore

private final class NoRedirectSessionDelegate: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(
        _ session: URLSession, task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}

public final class URLSessionHTTPExecutor: HTTPExecuting, @unchecked Sendable {
    private let session: URLSession

    public convenience init() {
        self.init(configuration: .ephemeral)
    }

    public init(configuration: URLSessionConfiguration) {
        configuration.httpShouldSetCookies = false
        configuration.httpCookieAcceptPolicy = .never
        configuration.httpCookieStorage = nil
        configuration.urlCache = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        configuration.httpMaximumConnectionsPerHost = 4
        session = URLSession(configuration: configuration, delegate: NoRedirectSessionDelegate(), delegateQueue: nil)
    }

    public func execute(_ request: HTTPRequest) async throws -> HTTPResponse {
        var urlRequest = URLRequest(url: request.url)
        urlRequest.httpMethod = request.method
        urlRequest.timeoutInterval = request.timeout
        urlRequest.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        urlRequest.httpShouldHandleCookies = false
        request.authorization.withValue {
            urlRequest.setValue("Bearer \($0)", forHTTPHeaderField: "Authorization")
        }
        if let key = request.idempotencyKey {
            urlRequest.setValue(key, forHTTPHeaderField: "Idempotency-Key")
        }
        if let body = request.body {
            urlRequest.setValue("application/json", forHTTPHeaderField: "Content-Type")
            urlRequest.httpBody = body
        }
        do {
            let (bytes, response) = try await session.bytes(for: urlRequest)
            guard let httpResponse = response as? HTTPURLResponse else { throw ClientError.serverUnavailable }
            var body = Data()
            body.reserveCapacity(min(request.maximumResponseBytes, 64 * 1_024))
            for try await byte in bytes {
                guard body.count < request.maximumResponseBytes else { throw ClientError.malformedPayload }
                body.append(byte)
            }
            return HTTPResponse(statusCode: httpResponse.statusCode, body: body)
        } catch let error as URLError {
            switch error.code {
            case .timedOut: throw ClientError.timeout
            case .notConnectedToInternet, .networkConnectionLost: throw ClientError.networkOffline
            default: throw ClientError.serverUnavailable
            }
        }
    }
}
