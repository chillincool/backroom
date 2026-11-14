import BackroomCore
import Foundation

/// One saved soju server and how to log in to it. Not secret; kept in
/// UserDefaults as part of a `ServerList`.
struct ServerSettings: Codable, Equatable, Identifiable, Sendable {
    enum Login: String, Codable, CaseIterable, Sendable {
        /// soju's own accounts (`auth internal`): username and password.
        case password
        /// soju's `auth oauth2`: a browser login at `issuer`.
        case singleSignOn
    }

    var id: UUID
    /// Shown in the server menu; the host when empty.
    var name: String
    var serverURL: String
    var login: Login
    /// The soju username, for `.password`.
    var username: String
    /// The OIDC issuer (Keycloak realm URL), for `.singleSignOn`.
    var issuer: String
    var nick: String

    init(id: UUID = UUID(), name: String = "", serverURL: String, login: Login,
         username: String = "", issuer: String = "", nick: String) {
        self.id = id
        self.name = name
        self.serverURL = serverURL
        self.login = login
        self.username = username
        self.issuer = issuer
        self.nick = nick
    }

    /// Settings saved before password logins existed were single sign-on.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decodeIfPresent(UUID.self, forKey: .id) ?? UUID()
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
        serverURL = try c.decode(String.self, forKey: .serverURL)
        login = try c.decodeIfPresent(Login.self, forKey: .login) ?? .singleSignOn
        username = try c.decodeIfPresent(String.self, forKey: .username) ?? ""
        issuer = try c.decodeIfPresent(String.self, forKey: .issuer) ?? ""
        nick = try c.decodeIfPresent(String.self, forKey: .nick) ?? ""
    }

    static let clientID = "backroom"
    static let redirectURI = "backroom://oauth/callback"
    static let callbackScheme = "backroom"
    /// `soju_roles` puts soju in the token's audience. `offline_access` gets a
    /// refresh token that outlives Keycloak's 30-minute idle browser session,
    /// so reconnecting after a while (sleep, network blip, soju restart)
    /// doesn't log you out.
    static let scopes = ["soju_roles", "offline_access"]

    /// What "Add Server" starts from.
    static var blank: ServerSettings { ServerSettings(serverURL: "", login: .password, nick: "") }

    /// The local e2e stack (`mise run e2e:up`), log in as alice / alice.
    static let e2e = ServerSettings(
        id: UUID(uuidString: "E2E00000-0000-0000-0000-000000000001")!, name: "Test stack (alice, SSO)",
        serverURL: "ws://127.0.0.1:18081/", login: .singleSignOn,
        issuer: "http://localhost:18080/realms/e2e", nick: "alice")

    /// The e2e stack's soju with its own accounts: dave / dave-secret.
    static let e2ePassword = ServerSettings(
        id: UUID(uuidString: "E2E00000-0000-0000-0000-000000000002")!, name: "Test stack (dave, password)",
        serverURL: "ws://127.0.0.1:28081/", login: .password, username: "dave", nick: "dave")

    /// Same server, login and nick as `other`: only the name differs.
    func connectsLike(_ other: ServerSettings?) -> Bool {
        guard var other else { return false }
        other.name = name
        return other == self
    }

    /// The name, or else the server's host.
    var displayName: String {
        if !name.isEmpty { return name }
        return URLComponents(string: serverURL)?.host ?? serverURL
    }

    /// Whether the fields the login method needs are filled in.
    var isComplete: Bool {
        guard !serverURL.isEmpty else { return false }
        switch login {
        case .password: return !username.isEmpty
        case .singleSignOn: return !issuer.isEmpty
        }
    }

    var clientConfig: ClientConfig {
        let method: LoginMethod = switch login {
        case .password: .password(username: username)
        case .singleSignOn:
            .oidc(issuer: issuer, clientId: Self.clientID, scopes: Self.scopes, redirectUri: Self.redirectURI)
        }
        // soju gives each network its own nick; this is only the default.
        let nick = !nick.isEmpty ? nick : !username.isEmpty ? username : "backroom"
        return ClientConfig(serverUrl: serverURL, login: method, nick: nick, clientVersion: Self.clientVersion)
    }

    /// Our CTCP VERSION reply: "Backroom 0.1.0 (macOS 26.3)".
    static var clientVersion: String {
        let version = Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "dev"
        let os = ProcessInfo.processInfo.operatingSystemVersion
        return "Backroom \(version) (macOS \(os.majorVersion).\(os.minorVersion))"
    }

    /// Keychain account for this login: tokens belong to one issuer and
    /// server, a password to one user and server.
    var keychainAccount: String {
        switch login {
        case .singleSignOn: "\(issuer) \(serverURL)"
        case .password: "password \(username) \(serverURL)"
        }
    }
}

/// The saved servers and which one the app uses. One session at a time;
/// each server keeps its own Keychain login.
struct ServerList: Codable, Equatable, Sendable {
    var servers: [ServerSettings] = []
    var selected: UUID?

    init(servers: [ServerSettings] = [], selected: UUID? = nil) {
        self.servers = servers
        self.selected = selected ?? servers.first?.id
    }

    /// The selected server, or the first.
    var current: ServerSettings? { servers.first { $0.id == selected } ?? servers.first }

    /// Adds `server`, or replaces the saved one with its id.
    mutating func save(_ server: ServerSettings) {
        if let i = servers.firstIndex(where: { $0.id == server.id }) {
            servers[i] = server
        } else {
            servers.append(server)
        }
        if current == nil || selected == nil { selected = server.id }
    }

    mutating func remove(_ id: UUID) {
        servers.removeAll { $0.id == id }
        if selected == id { selected = servers.first?.id }
    }

    private static let key = "servers"
    /// Before saved servers: one `ServerSettings`.
    private static let legacyKey = "serverSettings"

    static func load(from defaults: UserDefaults = .standard) -> ServerList {
        if let data = defaults.data(forKey: key),
            let list = try? JSONDecoder().decode(ServerList.self, from: data)
        {
            return list
        }
        if let data = defaults.data(forKey: legacyKey),
            let old = try? JSONDecoder().decode(ServerSettings.self, from: data)
        {
            return ServerList(servers: [old])
        }
        return ServerList()
    }

    func save(to defaults: UserDefaults) {
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
