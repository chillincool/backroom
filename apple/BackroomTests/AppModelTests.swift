import BackroomCore
import Foundation
import Testing

@testable import Backroom

/// Records calls instead of talking to soju. `nextEvent` returns nil, so the
/// model's event loop ends at once; tests feed events with `apply`.
final class FakeClient: BackroomClientProtocol, @unchecked Sendable {
    var loggedIn = false
    var calls: [String] = []

    func beginLogin() async throws -> String { "http://keycloak.test/auth?state=s" }
    func completeLogin(callbackUrl: String) async throws {
        calls.append("complete \(callbackUrl)")
        loggedIn = true
    }
    func connect() async throws {
        guard loggedIn else { throw BackroomError.NotLoggedIn(message: "none") }
        calls.append("connect")
    }
    func disconnect() { calls.append("disconnect") }
    func hasLogin() throws -> Bool { loggedIn }
    func join(network: String, channel: String) throws { calls.append("join \(network) \(channel)") }
    func loadHistory(network: String, target: String, before: Date?, limit: UInt32) throws {
        calls.append("history \(network) \(target) \(before == nil ? "latest" : "before")")
    }
    func logout() throws {
        loggedIn = false
        calls.append("logout")
    }
    func markRead(network: String, target: String, time: Date) throws {
        calls.append("read \(network) \(target) \(Int(time.timeIntervalSince1970))")
    }
    func nextEvent() async -> SessionEvent? { nil }
    func part(network: String, channel: String, reason: String?) throws {
        calls.append("part \(network) \(channel) \(reason ?? "-")")
    }
    func sendMessage(network: String, target: String, text: String) throws {
        calls.append("send \(network) \(target) \(text)")
    }
}

struct FakeAuthenticator: WebAuthenticator {
    var result: Result<URL, Error> = .success(URL(string: "backroom://oauth/callback?code=c&state=s")!)
    func authenticate(url: URL, callbackScheme: String) async throws -> URL { try result.get() }
    func cancel() {}
}

@MainActor
func makeModel(loggedIn: Bool = true, authenticator: FakeAuthenticator = FakeAuthenticator()) async
    -> (AppModel, FakeClient)
{
    let client = FakeClient()
    client.loggedIn = loggedIn
    let model = AppModel(settings: .e2e, authenticator: authenticator, makeClient: { _ in client })
    await model.start()
    return (model, client)
}

func msg(_ text: String, at seconds: TimeInterval, from sender: String = "erin", id: String? = nil,
         kind: MessageKind = .privmsg, own: Bool = false) -> ChatMessage {
    ChatMessage(id: id ?? "\(sender)-\(seconds)", time: Date(timeIntervalSince1970: seconds),
                sender: sender, kind: kind, text: text, own: own)
}

let net = Network(id: "1", name: "libera", upstreamState: "connected", attributes: [:])

@MainActor @Suite struct AppModelTests {
    @Test func startsLoggedOutWithoutAStoredLogin() async {
        let (model, client) = await makeModel(loggedIn: false)
        #expect(model.phase == .loggedOut)
        #expect(!client.calls.contains("connect"))
    }

    @Test func connectsWithAStoredLogin() async {
        let (model, client) = await makeModel()
        #expect(model.phase == .online)
        #expect(client.calls.contains("connect"))
    }

    @Test func loginCompletesAndConnects() async {
        let (model, client) = await makeModel(loggedIn: false)
        await model.logIn()
        #expect(client.calls.contains("complete backroom://oauth/callback?code=c&state=s"))
        #expect(client.calls.last == "connect")
        #expect(model.phase == .online)
    }

    @Test func cancelledLoginIsQuiet() async {
        let (model, _) = await makeModel(loggedIn: false, authenticator: FakeAuthenticator(result: .failure(CancellationError())))
        await model.logIn()
        #expect(model.phase == .loggedOut)
        #expect(model.notice == nil)
    }

    @Test func buildsNetworksAndBuffers() async {
        let (model, _) = await makeModel()
        model.apply(.networkUpdated(network: net))
        model.apply(.bufferAdded(network: "1", name: "#backroom", kind: .channel))
        model.apply(.bufferAdded(network: "1", name: "*", kind: .server))
        // A message for an unknown buffer creates it.
        model.apply(.message(network: "1", buffer: "erin", message: msg("hi", at: 10)))

        #expect(model.networks.map(\.network.name) == ["libera"])
        #expect(model.networks[0].buffers.map(\.key) == ["*", "#backroom", "erin"])
        #expect(model.buffers[BufferID(network: "1", name: "ERIN")]?.kind == .query)
    }

    @Test func unreadCountsOthersChatAfterTheMarker() async {
        let (model, _) = await makeModel()
        model.apply(.networkUpdated(network: net))
        let id = BufferID(network: "1", name: "#b")
        model.apply(.message(network: "1", buffer: "#b", message: msg("one", at: 10)))
        model.apply(.message(network: "1", buffer: "#b", message: msg("two", at: 20)))
        model.apply(.message(network: "1", buffer: "#b", message: msg("mine", at: 30, from: "alice", own: true)))
        model.apply(.message(network: "1", buffer: "#b", message: msg("", at: 40, kind: .join)))
        #expect(model.buffers[id]?.unread == 2)

        model.apply(.readMarker(network: "1", buffer: "#b", time: Date(timeIntervalSince1970: 15)))
        #expect(model.buffers[id]?.unread == 1)
    }

    @Test func selectingLoadsHistoryAndMarksRead() async {
        let (model, client) = await makeModel()
        model.apply(.networkUpdated(network: net))
        model.apply(.message(network: "1", buffer: "#b", message: msg("one", at: 10)))
        let id = BufferID(network: "1", name: "#b")

        model.select(id)
        #expect(client.calls.contains("history 1 #b latest"))
        #expect(client.calls.contains("read 1 #b 10"))
        #expect(model.buffers[id]?.unread == 0)

        // New messages in the open buffer are read at once…
        model.apply(.message(network: "1", buffer: "#b", message: msg("two", at: 20)))
        #expect(client.calls.last == "read 1 #b 20")
        // …but not while the app is in the background.
        model.isActive = false
        model.apply(.message(network: "1", buffer: "#b", message: msg("three", at: 30)))
        #expect(model.buffers[id]?.unread == 1)
        model.isActive = true
        #expect(client.calls.last == "read 1 #b 30")
    }

    @Test func staleReadMarkersDontBringUnreadBack() async {
        let (model, _) = await makeModel()
        model.apply(.networkUpdated(network: net))
        model.apply(.message(network: "1", buffer: "bob", message: msg("psst", at: 10, from: "bob")))
        let id = BufferID(network: "1", name: "bob")
        model.select(id)
        #expect(model.buffers[id]?.unread == 0)
        // soju's answer to the initial MARKREAD query: nothing read yet.
        model.apply(.readMarker(network: "1", buffer: "bob", time: nil))
        model.apply(.readMarker(network: "1", buffer: "bob", time: Date(timeIntervalSince1970: 5)))
        #expect(model.buffers[id]?.unread == 0)
        // Another device reading further still counts.
        model.apply(.message(network: "1", buffer: "bob", message: msg("more", at: 20, from: "bob")))
        model.select(nil)
        model.apply(.message(network: "1", buffer: "bob", message: msg("again", at: 30, from: "bob")))
        model.apply(.readMarker(network: "1", buffer: "bob", time: Date(timeIntervalSince1970: 30)))
        #expect(model.buffers[id]?.unread == 0)
    }

    @Test func historyMergesWithoutDuplicates() async {
        let (model, client) = await makeModel()
        model.apply(.networkUpdated(network: net))
        let id = BufferID(network: "1", name: "#b")
        model.apply(.message(network: "1", buffer: "#b", message: msg("live", at: 50, id: "m50")))
        model.select(id)
        model.apply(.history(network: "1", buffer: "#b", messages: (1...100).map {
            msg("h\($0)", at: TimeInterval($0), id: $0 == 50 ? "m50" : "h\($0)")
        }))
        let messages = model.buffers[id]?.messages ?? []
        #expect(messages.count == 100)
        #expect(messages.map(\.time) == messages.map(\.time).sorted())
        #expect(model.buffers[id]?.historyExhausted == false)

        model.loadOlder(id)
        #expect(client.calls.last == "history 1 #b before")
        // While loading, another request is ignored.
        let count = client.calls.count
        model.loadOlder(id)
        #expect(client.calls.count == count)

        model.apply(.history(network: "1", buffer: "#b", messages: [msg("old", at: 0, id: "h0")]))
        #expect(model.buffers[id]?.messages.first?.text == "old")
        #expect(model.buffers[id]?.historyExhausted == true)
    }

    @Test func composerSendsToTheSelectedBuffer() async {
        let (model, client) = await makeModel()
        model.apply(.networkUpdated(network: net))
        model.apply(.bufferAdded(network: "1", name: "#b", kind: .channel))
        model.select(BufferID(network: "1", name: "#b"))

        #expect(model.send("hello"))
        #expect(client.calls.last == "send 1 #b hello")
        #expect(model.send("/me waves"))
        #expect(client.calls.last == "send 1 #b \u{1}ACTION waves\u{1}")
        #expect(model.send("/join #rust"))
        #expect(client.calls.last == "join 1 #rust")
        #expect(model.send("/part bye"))
        #expect(client.calls.last == "part 1 #b bye")
        #expect(!model.send("/bogus"))
        #expect(model.notice != nil)
    }

    @Test func serverBufferRejectsMessages() async {
        let (model, client) = await makeModel()
        model.apply(.networkUpdated(network: net))
        model.apply(.bufferAdded(network: "1", name: "*", kind: .server))
        model.select(BufferID(network: "1", name: "*"))
        #expect(!model.send("hello"))
        #expect(!client.calls.contains { $0.hasPrefix("send") })
    }

    @Test func rejectedLoginLogsOut() async {
        let (model, _) = await makeModel()
        model.apply(.networkUpdated(network: net))
        model.apply(.connection(network: nil, state: .authFailed(reason: "token expired")))
        #expect(model.phase == .loggedOut)
        #expect(model.networks.isEmpty)
        #expect(model.notice?.contains("token expired") == true)
    }

    @Test func removingANetworkDropsItsBuffers() async {
        let (model, _) = await makeModel()
        model.apply(.networkUpdated(network: net))
        model.apply(.bufferAdded(network: "1", name: "#b", kind: .channel))
        model.select(BufferID(network: "1", name: "#b"))
        model.apply(.networkRemoved(id: "1"))
        #expect(model.networks.isEmpty)
        #expect(model.buffers.isEmpty)
        #expect(model.selection == nil)
    }
}
