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
        #expect(Composer.parse("/J #Rust", buffer: "#b") == .join("#Rust"))
        #expect(Composer.parse("/join", buffer: "#b") == .invalid("Usage: /join <#channel>"))
    }

    @Test func partDefaultsToCurrentBuffer() {
        #expect(Composer.parse("/part", buffer: "#b") == .part(channel: "#b", reason: nil))
        #expect(Composer.parse("/part bye all", buffer: "#b") == .part(channel: "#b", reason: "bye all"))
        #expect(Composer.parse("/part #x see ya", buffer: "#b") == .part(channel: "#x", reason: "see ya"))
    }

    @Test func joinAddsTheHash() {
        #expect(Composer.parse("/j rust", buffer: "#b") == .join("#rust"))
        #expect(Composer.parse("/join rust,#go,&local", buffer: "#b") == .join("#rust,#go,&local"))
    }

    @Test func shortcuts() {
        #expect(Composer.parse("/p", buffer: "#b") == .part(channel: "#b", reason: nil))
        #expect(Composer.parse("/m bob hi there", buffer: "#b") == .query(nick: "bob", text: "hi there"))
        #expect(Composer.parse("/msg bob", buffer: "#b") == .invalid("Usage: /msg <nick> <message>"))
        #expect(Composer.parse("/q bob", buffer: "#b") == .query(nick: "bob", text: nil))
        #expect(Composer.parse("/topic new topic", buffer: "#b") == .raw("TOPIC #b :new topic"))
        #expect(Composer.parse("/topic", buffer: "#b") == .raw("TOPIC #b"))
        #expect(Composer.parse("/quote PRIVMSG x :y", buffer: "#b") == .raw("PRIVMSG x :y"))
    }

    @Test func otherCommandsGoToTheServer() {
        #expect(Composer.parse("/whois alice", buffer: "#b") == .raw("WHOIS alice"))
        #expect(Composer.parse("/nick newme", buffer: "#b") == .raw("NICK newme"))
        #expect(Composer.parse("/away", buffer: "#b") == .raw("AWAY"))
    }
}
