import Foundation
import Security

/// Keeps the login tokens (an opaque value serialized by the Rust core) in
/// one generic-password Keychain item. Never log the value.
public final class KeychainSecretStore: SecretStore, @unchecked Sendable {
    // Immutable after init; Keychain calls are thread-safe.
    private let service: String
    private let account: String

    /// - Parameters:
    ///   - service: Keychain service, e.g. the app's bundle ID.
    ///   - account: Which login, e.g. the soju server URL.
    public init(service: String, account: String) {
        self.service = service
        self.account = account
    }

    private var query: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    public func load() throws -> String? {
        var item = query
        item[kSecReturnData as String] = true
        item[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(item as CFDictionary, &result)
        switch status {
        case errSecSuccess:
            guard let data = result as? Data, let value = String(data: data, encoding: .utf8) else {
                throw BackroomError.Store(message: "Keychain item is not UTF-8")
            }
            return value
        case errSecItemNotFound:
            return nil
        default:
            throw Self.error("read", status)
        }
    }

    public func save(value: String) throws {
        let data = Data(value.utf8)
        let update: [String: Any] = [kSecValueData as String: data]
        var status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
        if status == errSecItemNotFound {
            var item = query
            item[kSecValueData as String] = data
            item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            status = SecItemAdd(item as CFDictionary, nil)
        }
        guard status == errSecSuccess else { throw Self.error("save", status) }
    }

    public func clear() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw Self.error("delete", status)
        }
    }

    private static func error(_ action: String, _ status: OSStatus) -> BackroomError {
        let text = SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)"
        return BackroomError.Store(message: "Keychain \(action) failed: \(text)")
    }
}
