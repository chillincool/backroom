import BackroomCore
import SwiftUI

/// A WHOIS reply as a card.
struct WhoisView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let network: String
    let info: WhoisInfo

    var body: some View {
        Form {
            Section {
                row("Name", info.realname)
                row("Address", [info.user, info.host].allSatisfy { $0 != nil }
                    ? "\(info.user!)@\(info.host!)" : nil)
                row("Account", info.account)
                row("Away", info.away)
                if info.operator { row("Status", "IRC operator") }
                if info.secure { row("Connection", "Secure (TLS)") }
            }
            if !info.channels.isEmpty {
                Section("Channels") {
                    Text(info.channels.joined(separator: "  "))
                        .textSelection(.enabled)
                }
            }
            Section {
                row("Server", [info.server, info.serverInfo.map { "(\($0))" }].compactMap { $0 }.joined(separator: " "))
                if let idle = info.idle {
                    row("Idle", Duration.seconds(idle).formatted(.units(allowed: [.days, .hours, .minutes, .seconds], width: .wide, maximumUnitCount: 2)))
                }
                if let signon = info.signon {
                    row("Connected since", signon.formatted(date: .abbreviated, time: .shortened))
                }
                ForEach(info.other, id: \.self) { Text($0).foregroundStyle(.secondary).textSelection(.enabled) }
            }
        }
        .formStyle(.grouped)
        .frame(width: 420)
        .navigationTitle(info.nick)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Close") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Send Message") {
                    model.openQuery(network: network, nick: info.nick)
                    dismiss()
                }
            }
        }
    }

    @ViewBuilder private func row(_ title: String, _ value: String?) -> some View {
        if let value, !value.isEmpty {
            LabeledContent(title) {
                Text(value).textSelection(.enabled)
            }
        }
    }
}
