import BackroomCore
import Foundation

/// Which soju and Keycloak to use. Not secret; kept in UserDefaults.
struct ServerSettings: Codable, Equatable, Sendable {
    var serverURL: String
    var issuer: String
    var nick: String

    static let clientID = "backroom"
    static let redirectURI = "backroom://oauth/callback"
    static let callbackScheme = "backroom"
    /// `soju_roles` puts soju in the token's audience. `offline_access` gets a
    /// refresh token that outlives Keycloak's 30-minute idle browser session,
    /// so reconnecting after a while (sleep, network blip, soju restart)
    /// doesn't log you out.
    static let scopes = ["soju_roles", "offline_access"]

    /// The local e2e stack (`mise run e2e:up`), log in as alice / alice.
    static let e2e = ServerSettings(
        serverURL: "ws://127.0.0.1:18081/",
        issuer: "http://localhost:18080/realms/e2e",
        nick: "alice")

    var clientConfig: ClientConfig {
        ClientConfig(
            serverUrl: serverURL, issuer: issuer, clientId: Self.clientID,
            scopes: Self.scopes, redirectUri: Self.redirectURI, nick: nick,
            clientVersion: Self.clientVersion)
    }

    /// Our CTCP VERSION reply: "Backroom 0.1.0 (macOS 26.3)".
    static var clientVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"
        let os = ProcessInfo.processInfo.operatingSystemVersion
        return "Backroom \(version) (macOS \(os.majorVersion).\(os.minorVersion))"
    }

    /// Keychain account for this login: tokens belong to one issuer and server.
    var keychainAccount: String { "\(issuer) \(serverURL)" }

    private static let key = "serverSettings"

    static func load(from defaults: UserDefaults = .standard) -> ServerSettings {
        guard let data = defaults.data(forKey: key),
            let settings = try? JSONDecoder().decode(ServerSettings.self, from: data)
        else { return .e2e }
        return settings
    }

    func save(to defaults: UserDefaults = .standard) {
        if let data = try? JSONEncoder().encode(self) {
            defaults.set(data, forKey: Self.key)
        }
    }
}

/// What the network form edits, as soju network attributes.
struct NetworkFields: Equatable {
    var name: String
    var host: String
    var port: Int
    var tls: Bool
    var nick: String

    func attributes(keepEmpty: Bool) -> [String: String] {
        var attributes = ["host": host, "port": String(port), "tls": tls ? "1" : "0"]
        for (key, value) in [("name", name), ("nickname", nick)] where keepEmpty || !value.isEmpty {
            attributes[key] = value
        }
        return attributes
    }
}
