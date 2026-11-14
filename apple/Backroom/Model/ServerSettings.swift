import BackroomCore
import Foundation

/// Which soju to use and how to log in. Not secret; kept in UserDefaults.
struct ServerSettings: Codable, Equatable, Sendable {
    enum Login: String, Codable, CaseIterable, Sendable {
        /// soju's own accounts (`auth internal`): username and password.
        case password
        /// soju's `auth oauth2`: a browser login at `issuer`.
        case singleSignOn
    }

    var serverURL: String
    var login: Login
    /// The soju username, for `.password`.
    var username: String
    /// The OIDC issuer (Keycloak realm URL), for `.singleSignOn`.
    var issuer: String
    var nick: String

    init(serverURL: String, login: Login, username: String = "", issuer: String = "", nick: String) {
        self.serverURL = serverURL
        self.login = login
        self.username = username
        self.issuer = issuer
        self.nick = nick
    }

    /// Settings saved before password logins existed were single sign-on.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
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

    /// The local e2e stack (`mise run e2e:up`), log in as alice / alice.
    static let e2e = ServerSettings(
        serverURL: "ws://127.0.0.1:18081/", login: .singleSignOn,
        issuer: "http://localhost:18080/realms/e2e", nick: "alice")

    /// The e2e stack's soju with its own accounts: dave / dave-secret.
    static let e2ePassword = ServerSettings(
        serverURL: "ws://127.0.0.1:28081/", login: .password, username: "dave", nick: "dave")

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
        let nick = nick.isEmpty && login == .password ? username : nick
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
