import AppKit

/// Logs in through the default browser (RFC 8252 "external user-agent"):
/// opens the login page there, and macOS hands the `backroom://` redirect
/// back to the app, which passes it to `handle(_:)`.
///
/// Not `ASWebAuthenticationSession`: it delegates to the default browser,
/// and Firefox fails those requests at once, reporting them as cancelled.
@MainActor
final class BrowserLogin: WebAuthenticator {
    private var pending: (continuation: CheckedContinuation<URL, Error>, scheme: String)?

    func authenticate(url: URL, callbackScheme: String) async throws -> URL {
        cancel()
        return try await withCheckedThrowingContinuation { continuation in
            pending = (continuation, callbackScheme)
            if !NSWorkspace.shared.open(url) {
                pending = nil
                continuation.resume(throwing: URLError(.cannotOpenFile))
            }
        }
    }

    /// Takes a URL the app was asked to open. Returns false if it isn't the
    /// redirect of a login in progress.
    @discardableResult
    func handle(_ url: URL) -> Bool {
        guard let pending, url.scheme?.lowercased() == pending.scheme else { return false }
        self.pending = nil
        pending.continuation.resume(returning: url)
        NSApp.activate()
        return true
    }

    func cancel() {
        pending?.continuation.resume(throwing: CancellationError())
        pending = nil
    }
}
