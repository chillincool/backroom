import AppKit
import SwiftUI
import UserNotifications

/// When Backroom posts macOS notifications.
struct NotificationSettings: View {
    @Environment(Preferences.self) private var prefs
    @State private var status: UNAuthorizationStatus?

    var body: some View {
        @Bindable var prefs = prefs
        Form {
            Section {
                Toggle("Notify me about mentions and private messages", isOn: $prefs.notifications)
                Toggle("Also every message in channels", isOn: $prefs.notifyAllMessages)
                    .disabled(!prefs.notifications)
                Toggle("Play a sound", isOn: $prefs.notificationSound)
                    .disabled(!prefs.notifications)
            } footer: {
                Text("Only for conversations that aren't on screen. Highlight words count as mentions (Settings › Highlights). The Dock icon counts unread mentions and private messages.")
                    .foregroundStyle(.secondary)
            }
            Section {
                LabeledContent("Permission") {
                    HStack {
                        Text(statusText).foregroundStyle(.secondary)
                        if status == .notDetermined {
                            Button("Ask Now") {
                                Task {
                                    _ = try? await UNUserNotificationCenter.current()
                                        .requestAuthorization(options: [.alert, .sound, .badge])
                                    await refresh()
                                }
                            }
                        } else {
                            Button("Open System Settings") {
                                NSWorkspace.shared.open(
                                    URL(string: "x-apple.systempreferences:com.apple.Notifications-Settings.extension")!)
                            }
                        }
                    }
                }
            } footer: {
                Text("The Dock badge also needs \"Badge application icon\" allowed there.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .task { await refresh() }
    }

    private func refresh() async {
        status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    private var statusText: String {
        switch status {
        case .authorized, .provisional, .ephemeral: "Allowed"
        case .denied: "Blocked in System Settings"
        case .notDetermined: "Not asked yet"
        case nil: "…"
        @unknown default: "Unknown"
        }
    }
}
