import Testing

@testable import Backroom

@Suite struct ComposerTests {
    @Test func plainTextIsAMessage() {
        #expect(Composer.parse("hello there", buffer: "#b") == .message("hello there"))
    }

    @Test func blankIsIgnored() {
        #expect(Composer.parse("   ", buffer: "#b") == nil)
    }

    @Test func doubleSlashSendsASlash() {
        #expect(Composer.parse("//shrug", buffer: "#b") == .message("/shrug"))
    }

    @Test func me() {
        #expect(Composer.parse("/me waves", buffer: "#b") == .action("waves"))
        #expect(Composer.parse("/ME waves", buffer: "#b") == .action("waves"))
        #expect(Composer.parse("/me", buffer: "#b") == .invalid("Usage: /me <action>"))
        #expect(Composer.actionText("waves") == "\u{1}ACTION waves\u{1}")
    }

    @Test func join() {
        #expect(Composer.parse("/join #rust", buffer: "#b") == .join("#rust"))
        #expect(Composer.parse("/j #rust extra", buffer: "#b") == .join("#rust"))
        #expect(Composer.parse("/join", buffer: "#b") == .invalid("Usage: /join <#channel>"))
    }

    @Test func partDefaultsToCurrentBuffer() {
        #expect(Composer.parse("/part", buffer: "#b") == .part(channel: "#b", reason: nil))
        #expect(Composer.parse("/part bye all", buffer: "#b") == .part(channel: "#b", reason: "bye all"))
        #expect(Composer.parse("/part #x see ya", buffer: "#b") == .part(channel: "#x", reason: "see ya"))
    }

    @Test func unknownCommand() {
        guard case .invalid = Composer.parse("/frobnicate", buffer: "#b") else {
            Issue.record("expected invalid")
            return
        }
    }
}
