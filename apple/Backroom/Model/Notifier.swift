import AppKit
import BackroomCore
import UserNotifications

/// Posts macOS notifications for messages and opens their buffer when one
/// is clicked. Asks for permission the first time it's needed.
@MainActor
final class Notifier: NSObject, @preconcurrency UNUserNotificationCenterDelegate {
    /// Shows a buffer; set by the app.
    var open: ((BufferID) -> Void)?
    private var asked = false

    private var center: UNUserNotificationCenter { .current() }

    func post(_ message: ChatMessage, in buffer: BufferModel, network: String, sound: Bool) {
        let content = UNMutableNotificationContent()
        let text = stripFormatting(text: message.text)
        switch buffer.kind {
        case .query:
            content.title = message.sender
            content.body = message.kind == .action ? "* \(message.sender) \(text)" : text
        default:
            content.title = buffer.name
            content.subtitle = network
            content.body = message.kind == .action ? "* \(message.sender) \(text)" : "\(message.sender): \(text)"
        }
        content.threadIdentifier = "\(buffer.id.network)|\(buffer.id.key)"
        content.userInfo = ["network": buffer.id.network, "buffer": buffer.name]
        if sound { content.sound = .default }
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        Task {
            if !asked {
                asked = true
                _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
            }
            try? await center.add(request)
        }
    }

    // Shown even while Backroom is frontmost: we only post for buffers
    // that aren't in view.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let info = response.notification.request.content.userInfo
        guard let network = info["network"] as? String, let buffer = info["buffer"] as? String else { return }
        NSApp.activate()
        open?(BufferID(network: network, name: buffer))
    }
}
