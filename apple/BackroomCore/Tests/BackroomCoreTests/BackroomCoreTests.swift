import Foundation
import Testing

@testable import BackroomCore

/// In-memory stand-in for the Keychain.
final class MemoryStore: SecretStore, @unchecked Sendable {
    var value: String?
    var failLoad = false

    func load() throws -> String? {
        if failLoad { throw BackroomError.Store(message: "keychain locked") }
        return value
    }

    func save(value: String) throws { self.value = value }
    func clear() throws { value = nil }
}

/// Nothing listens on port 1, so network calls fail fast.
func config(serverUrl: String = "ws://127.0.0.1:1/socket") -> ClientConfig {
    ClientConfig(
        serverUrl: serverUrl, issuer: "http://127.0.0.1:1/realms/none",
        clientId: "backroom", scopes: ["soju_roles"],
        redirectUri: "backroom://oauth/callback", nick: "dave")
}

@Test func newClientHasNoLogin() throws {
    let client = try BackroomClient(config: config(), secretStore: MemoryStore())
    #expect(try client.hasLogin() == false)
}

@Test func commandsNeedASession() throws {
    let client = try BackroomClient(config: config(), secretStore: MemoryStore())
    #expect(throws: BackroomError.NotConnected) {
        try client.join(network: "1", channel: "#backroom")
    }
}

@Test func connectNeedsALogin() async throws {
    let client = try BackroomClient(config: config(), secretStore: MemoryStore())
    await #expect(throws: BackroomError.self) { try await client.connect() }
    #expect(await client.nextEvent() == nil)
}

@Test func beginLoginReportsDiscoveryFailure() async throws {
    let client = try BackroomClient(config: config(), secretStore: MemoryStore())
    do {
        _ = try await client.beginLogin()
        Issue.record("beginLogin should fail without a Keycloak")
    } catch BackroomError.Auth(let message) {
        #expect(message.contains("discovery"))
    }
}

@Test func storeErrorsCrossTheBoundary() throws {
    let store = MemoryStore()
    store.failLoad = true
    let client = try BackroomClient(config: config(), secretStore: store)
    #expect(throws: BackroomError.Store(message: "keychain locked")) {
        try client.hasLogin()
    }
}

@Test func rejectsUnsupportedServerURL() {
    #expect(throws: BackroomError.self) {
        try BackroomClient(config: config(serverUrl: "irc://irc.example.net"), secretStore: MemoryStore())
    }
}

@Test func stripsFormatting() {
    #expect(stripFormatting(text: "\u{02}bold\u{02} \u{03}4red") == "bold red")
}

@Test func keychainRoundTrip() throws {
    let store = KeychainSecretStore(service: "net.chillincool.backroom.tests", account: UUID().uuidString)
    defer { try? store.clear() }
    #expect(try store.load() == nil)
    try store.save(value: "first")
    try store.save(value: "second")
    #expect(try store.load() == "second")
    try store.clear()
    #expect(try store.load() == nil)
}
