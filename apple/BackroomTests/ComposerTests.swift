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
        #expect(Composer.parse("/join #rust", buffer: "#b") == .join("#rust", key: nil))
        #expect(Composer.parse("/j #rust sesame extra", buffer: "#b") == .join("#rust", key: "sesame"))
        #expect(Composer.parse("/J #Rust", buffer: "#b") == .join("#Rust", key: nil))
        #expect(Composer.parse("/join", buffer: "#b") == .invalid("Usage: /join <#channel> [key]"))
    }

    @Test func partDefaultsToCurrentBuffer() {
        #expect(Composer.parse("/part", buffer: "#b") == .part(channel: "#b", reason: nil))
        #expect(Composer.parse("/part bye all", buffer: "#b") == .part(channel: "#b", reason: "bye all"))
        #expect(Composer.parse("/part #x see ya", buffer: "#b") == .part(channel: "#x", reason: "see ya"))
    }

    @Test func joinAddsTheHash() {
        #expect(Composer.parse("/j rust", buffer: "#b") == .join("#rust", key: nil))
        #expect(Composer.parse("/join rust,#go,&local", buffer: "#b") == .join("#rust,#go,&local", key: nil))
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

    @Test func ircCommands() {
        #expect(Composer.parse("/whois alice", buffer: "#b") == .raw("WHOIS alice"))
        #expect(Composer.parse("/nick newme", buffer: "#b") == .raw("NICK newme"))
        #expect(Composer.parse("/away", buffer: "#b") == .raw("AWAY :Away"))
        #expect(Composer.parse("/away lunch", buffer: "#b") == .raw("AWAY :lunch"))
        #expect(Composer.parse("/back", buffer: "#b") == .raw("AWAY"))
        #expect(Composer.parse("/invite bob", buffer: "#b") == .raw("INVITE bob #b"))
        #expect(Composer.parse("/invite bob #x", buffer: "bob") == .raw("INVITE bob #x"))
        #expect(Composer.parse("/notice bob psst", buffer: "#b") == .raw("NOTICE bob :psst"))
        #expect(Composer.parse("/ctcp bob version", buffer: "#b") == .raw("PRIVMSG bob :\u{1}VERSION\u{1}"))
        #expect(Composer.parse("/motd", buffer: "*") == .raw("MOTD"))
        #expect(Composer.parse("/list", buffer: "*") == .listChannels)
        #expect(Composer.parse("/ignore spam", buffer: "*") == .ignore("spam"))
    }

    @Test func channelModeration() {
        #expect(Composer.parse("/kick bob", buffer: "#b") == .raw("KICK #b bob"))
        #expect(Composer.parse("/kick bob be nice", buffer: "#b") == .raw("KICK #b bob :be nice"))
        #expect(Composer.parse("/ban bob", buffer: "#b") == .raw("MODE #b +b bob!*@*"))
        #expect(Composer.parse("/unban *!*@spam.example", buffer: "#b") == .raw("MODE #b -b *!*@spam.example"))
        #expect(Composer.parse("/op alice bob", buffer: "#b") == .raw("MODE #b +oo alice bob"))
        #expect(Composer.parse("/devoice carol", buffer: "#b") == .raw("MODE #b -v carol"))
        #expect(Composer.parse("/mode +m", buffer: "#b") == .raw("MODE #b +m"))
        #expect(Composer.parse("/mode me +i", buffer: "#b") == .raw("MODE me +i"))
        #expect(Composer.parse("/cycle", buffer: "#b") == .cycle(channel: "#b"))
        // Channel commands outside a channel.
        #expect(Composer.parse("/kick bob", buffer: "bob") == .invalid("/kick works in a channel."))
        #expect(Composer.parse("/part", buffer: "*") == .invalid("/part works in a channel."))
    }

    @Test func tabCompletesAndCycles() {
        let nicks = ["alice", "Albert", "bob"]
        var c = Completion.start("al", nicks: nicks, channels: [])
        #expect(c?.line == "alice: ")
        c?.next()
        #expect(c?.line == "Albert: ")
        c?.next()
        #expect(c?.line == "alice: ")
        #expect(Completion.start("hi b", nicks: nicks, channels: [])?.line == "hi bob ")
        #expect(Completion.start("see #ru", nicks: nicks, channels: ["#rust"])?.line == "see #rust ")
        #expect(Completion.start("/wh", nicks: nicks, channels: [])?.line == "/whois ")
        #expect(Completion.start("zz", nicks: nicks, channels: []) == nil)
        #expect(Completion.start("hi ", nicks: nicks, channels: []) == nil)
    }
}
