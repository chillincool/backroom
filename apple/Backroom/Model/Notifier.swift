import AppKit
import BackroomCore
import UserNotifications

/// Posts macOS notifications for messages and opens their buffer when one
/// is clicked. Permission is asked for at launch: asking only when the
/// first message arrives loses that message (macOS shows the request as a
/// notification of its own, which leads to System Settings).
@MainActor
final class Notifier: NSObject, @preconcurrency UNUserNotificationCenterDelegate {
    /// Shows a buffer; set by the app.
    var open: ((BufferID) -> Void)?

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
        Task { try? await center.add(request) }
    }

    /// Asks macOS for permission, if it hasn't been asked yet.
    func requestPermission() async {
        _ = try? await center.requestAuthorization(options: [.alert, .sound, .badge])
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
