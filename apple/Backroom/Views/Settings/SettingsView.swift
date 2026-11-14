import SwiftUI

/// The Settings window (⌘,).
struct SettingsView: View {
    @Environment(Preferences.self) private var prefs
    @Environment(\.colorScheme) private var colorScheme
    @AppStorage("settingsTab") private var tab = Tab.account

    enum Tab: String {
        case account, networks, appearance, messages
    }

    var body: some View {
        TabView(selection: $tab) {
            AccountSettings()
                .tabItem { Label("Account", systemImage: "person.crop.circle") }
                .tag(Tab.account)
            NetworkSettings()
                .tabItem { Label("Networks", systemImage: "network") }
                .tag(Tab.networks)
            AppearanceSettings()
                .tabItem { Label("Appearance", systemImage: "paintpalette") }
                .tag(Tab.appearance)
            MessageSettings()
                .tabItem { Label("Messages", systemImage: "text.bubble") }
                .tag(Tab.messages)
        }
        .frame(width: 640)
        .environment(\.theme, prefs.theme(dark: colorScheme == .dark))
    }
}
