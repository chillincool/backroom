import Foundation
import Testing

@testable import Backroom

@MainActor @Suite struct BrowserLoginTests {
    @Test func ignoresURLsWithoutALoginInProgress() {
        #expect(!BrowserLogin().handle(URL(string: "backroom://oauth/callback?code=c")!))
    }
}
