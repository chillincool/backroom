import BackroomCore
import Foundation

/// One saved soju server and how to log in to it, or the built-in "No
/// bouncer" entry (`.standalone`). Not secret; kept in
/// UserDefaults as part of a `ServerList`.
struct ServerSettings: Codable, Equatable, Identifiable, Sendable {
    enum Login: String, Codable, CaseIterable, Sendable {
        /// soju's own accounts (`auth internal`): username and password.
        case password
        /// soju's `auth oauth2`: a browser login at `issuer`.
        case singleSignOn
        /// No bouncer: Backroom connects to each network itself and keeps
        /// history on this Mac. Always in the list, as `ServerSettings.standalone`.
        case standalone
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

    /// Without a bouncer. Only the nick (the default for networks) can be
    /// changed.
    static let standalone = ServerSettings(
        id: UUID(uuidString: "00000000-0000-0000-0000-00000000B0B0")!, name: "No bouncer",
        serverURL: "", login: .standalone, nick: "")

    var isStandalone: Bool { login == .standalone }

    /// The standalone history database.
    static var databaseURL: URL { Preferences.dataFolder.appending(path: "standalone.sqlite") }

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
        if isStandalone { return true }
        guard !serverURL.isEmpty else { return false }
        switch login {
        case .password: return !username.isEmpty
        case .singleSignOn: return !issuer.isEmpty
        case .standalone: return true
        }
    }

    /// The core's config; `textLogs` is where standalone mode also writes
    /// plain-text logs (nil: off).
    func clientConfig(textLogs: String? = nil) -> ClientConfig {
        let method: LoginMethod = switch login {
        case .password: .password(username: username)
        case .singleSignOn:
            .oidc(issuer: issuer, clientId: Self.clientID, scopes: Self.scopes, redirectUri: Self.redirectURI)
        case .standalone:
            .standalone(database: Self.databaseURL.path(percentEncoded: false), textLogs: textLogs)
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
    /// server, a password to one user and server. Standalone mode keeps all
    /// network passwords and channel keys in one item.
    var keychainAccount: String {
        switch login {
        case .singleSignOn: "\(issuer) \(serverURL)"
        case .password: "password \(username) \(serverURL)"
        case .standalone: "standalone"
        }
    }
}

/// The saved servers and which one the app uses. One session at a time;
/// each server keeps its own Keychain login.
struct ServerList: Codable, Equatable, Sendable {
    var servers: [ServerSettings] = []
    var selected: UUID?

    /// "No bouncer" is always there, last.
    init(servers: [ServerSettings] = [], selected: UUID? = nil) {
        self.servers = servers
        self.selected = selected ?? servers.first?.id
        addStandalone()
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        servers = try c.decode([ServerSettings].self, forKey: .servers)
        selected = try c.decodeIfPresent(UUID.self, forKey: .selected)
        addStandalone()
    }

    private mutating func addStandalone() {
        if !servers.contains(where: \.isStandalone) { servers.append(.standalone) }
    }

    /// The soju servers, without "No bouncer".
    var bouncers: [ServerSettings] { servers.filter { !$0.isStandalone } }
    var standalone: ServerSettings { servers.first(where: \.isStandalone) ?? .standalone }

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
        servers.removeAll { $0.id == id && !$0.isStandalone }
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
    /// Without a bouncer: how to log in to the network. nil leaves it alone
    /// (soju keeps its own).
    var login: NetworkLogin? = nil

    func attributes(keepEmpty: Bool) -> [String: String] {
        var attributes = ["host": host, "port": String(port), "tls": tls ? "1" : "0"]
        for (key, value) in [("name", name), ("nickname", nick)] where keepEmpty || !value.isEmpty {
            attributes[key] = value
        }
        if let login { attributes.merge(login.attributes) { $1 } }
        return attributes
    }
}

/// How a standalone network logs in, as the core's attributes. Passwords go
/// to the Keychain; the network only says which ones are saved (`pass-set`, …).
struct NetworkLogin: Equatable {
    enum Method: String, CaseIterable, Identifiable {
        case none, sasl, serverPassword, nickServ
        var id: Self { self }

        var title: String {
            switch self {
            case .none: "None"
            case .sasl: "SASL (account login)"
            case .serverPassword: "Server password"
            case .nickServ: "NickServ"
            }
        }

        /// The secret attribute this method uses.
        var secret: String? {
            switch self {
            case .none: nil
            case .sasl: "sasl-password"
            case .serverPassword: "pass"
            case .nickServ: "nickserv-password"
            }
        }
    }

    var method: Method = .none
    /// The SASL account; the nick when empty.
    var username = ""
    /// A new password; empty keeps the saved one.
    var password = ""

    /// The login saved on `attributes`, without the password.
    init(attributes: [String: String]) {
        method = Method.allCases.first { m in m.secret.map { attributes["\($0)-set"] == "1" } ?? false } ?? .none
        username = attributes["sasl-username"] ?? ""
    }

    init(method: Method = .none, username: String = "", password: String = "") {
        self.method = method
        self.username = username
        self.password = password
    }

    /// The other methods' passwords are cleared; this one's is set if typed.
    var attributes: [String: String] {
        var attributes: [String: String] = [:]
        for other in Method.allCases where other != method {
            if let secret = other.secret { attributes[secret] = "" }
        }
        if let secret = method.secret, !password.isEmpty { attributes[secret] = password }
        attributes["sasl-username"] = method == .sasl ? username : ""
        return attributes
    }

    /// Whether this can be saved: a new method needs its password.
    func isComplete(saved: NetworkLogin?) -> Bool {
        method == .none || !password.isEmpty || saved?.method == method
    }
}
