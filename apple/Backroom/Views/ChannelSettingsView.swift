import BackroomCore
import SwiftUI

/// A channel's topic, modes and ban list. Changes need ops; others see them
/// read-only.
struct ChannelSettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    let network: String
    let channel: String
    @State private var topic = ""
    @State private var flags: Set<Character> = []
    @State private var key = ""
    @State private var limit = ""
    @State private var newBan = ""
    @State private var loaded = false

    /// The usual simple modes, with what they mean.
    static let simpleModes: [(Character, String)] = [
        ("n", "No messages from outside the channel"),
        ("t", "Only operators change the topic"),
        ("i", "Invite only"),
        ("m", "Moderated: only voiced people and operators speak"),
        ("s", "Secret: hidden from the channel list and WHOIS"),
    ]

    var body: some View {
        let settings = model.channelSettings
        let buffer = model.buffers[BufferID(network: network, name: channel)]
        let canEdit = buffer.flatMap { model.ownPrefix(in: $0) }?.contains { "~&@%".contains($0) } ?? false
        Form {
            Section("Topic") {
                TextField("Topic", text: $topic, axis: .vertical)
                    .lineLimit(2...6)
                    .disabled(!canEdit && flags.contains("t"))
            }
            Section {
                if settings?.modes == nil {
                    ProgressView().controlSize(.small)
                } else {
                    ForEach(Self.simpleModes, id: \.0) { mode, title in
                        Toggle(title, isOn: Binding(
                            get: { flags.contains(mode) },
                            set: { if $0 { flags.insert(mode) } else { flags.remove(mode) } }))
                    }
                    TextField("Key (password)", text: $key, prompt: Text("None"))
                    TextField("User limit", text: $limit, prompt: Text("None"))
                    if let other = otherModes(settings), !other.isEmpty {
                        LabeledContent("Other modes", value: other)
                    }
                }
            } header: {
                Text("Modes")
            } footer: {
                if !canEdit {
                    Text("Only operators can change these.").foregroundStyle(.secondary)
                }
            }
            .disabled(!canEdit)
            Section("Bans") {
                if let bans = settings?.bans {
                    if bans.isEmpty { Text("Nobody is banned.").foregroundStyle(.secondary) }
                    ForEach(bans, id: \.mask) { ban in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(ban.mask).textSelection(.enabled)
                                if let by = ban.setBy {
                                    Text("by \(by)" + (ban.setAt.map { ", \($0.formatted(date: .abbreviated, time: .shortened))" } ?? ""))
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                            if canEdit {
                                Button("Remove") { raw("MODE \(channel) -b \(ban.mask)"); reload() }
                            }
                        }
                    }
                    if canEdit {
                        HStack {
                            TextField("Ban a nick or mask", text: $newBan)
                            Button("Ban") {
                                raw("MODE \(channel) +b \(Composer.banMask(newBan.trimmingCharacters(in: .whitespaces)))")
                                newBan = ""
                                reload()
                            }
                            .disabled(newBan.trimmingCharacters(in: .whitespaces).isEmpty)
                        }
                    }
                } else {
                    ProgressView().controlSize(.small)
                }
            }
        }
        .formStyle(.grouped)
        .frame(width: 480, height: 560)
        .navigationTitle("\(channel) Settings")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Close") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button("Apply") {
                    apply(settings, buffer: buffer)
                    dismiss()
                }
                .disabled(!loaded)
            }
        }
        .onAppear { topic = buffer?.topic.map { stripFormatting(text: $0) } ?? "" }
        .onChange(of: settings?.modes, initial: true) { load(settings) }
    }

    private func raw(_ line: String) { model.sendRaw(network: network, line) }

    private func reload() {
        Task {
            try? await Task.sleep(for: .milliseconds(500))
            model.openChannelSettings(network: network, channel: channel)
        }
    }

    /// Fills the form from RPL_CHANNELMODEIS.
    private func load(_ settings: AppModel.ChannelSettings?) {
        guard let settings, let modes = settings.modes, !loaded else { return }
        let parsed = Self.parse(modes, settings.params)
        flags = parsed.flags
        key = parsed.key ?? ""
        limit = parsed.limit ?? ""
        loaded = true
    }

    static func parse(_ modes: String, _ params: [String]) -> (flags: Set<Character>, key: String?, limit: String?) {
        var flags = Set<Character>()
        var args = params.makeIterator()
        var key: String?, limit: String?
        for mode in modes where mode != "+" {
            switch mode {
            case "k": key = args.next()
            case "l": limit = args.next()
            default: flags.insert(mode)
            }
        }
        return (flags, key, limit)
    }

    private func otherModes(_ settings: AppModel.ChannelSettings?) -> String? {
        guard let modes = settings?.modes else { return nil }
        let simple = Set(Self.simpleModes.map(\.0) + ["k", "l", "+"])
        let other = modes.filter { !simple.contains($0) }
        return other.isEmpty ? nil : "+" + other
    }

    /// Sends what changed: TOPIC and one MODE line.
    private func apply(_ settings: AppModel.ChannelSettings?, buffer: BufferModel?) {
        let oldTopic = buffer?.topic.map { stripFormatting(text: $0) } ?? ""
        let newTopic = topic.trimmingCharacters(in: .whitespacesAndNewlines).replacingOccurrences(of: "\n", with: " ")
        if newTopic != oldTopic { raw("TOPIC \(channel) :\(newTopic)") }
        guard let settings, let modes = settings.modes else { return }
        if let line = Self.modeChange(
            channel: channel, from: Self.parse(modes, settings.params),
            to: (flags, key.trimmingCharacters(in: .whitespaces), limit.trimmingCharacters(in: .whitespaces))) {
            raw(line)
        }
    }

    /// The MODE line that turns `old` into `new`, or nil if nothing changed.
    static func modeChange(
        channel: String, from old: (flags: Set<Character>, key: String?, limit: String?),
        to new: (flags: Set<Character>, key: String, limit: String)
    ) -> String? {
        let simple = Set(simpleModes.map(\.0))
        var add = "", remove = "", args: [String] = []
        for (mode, _) in simpleModes {
            let was = old.flags.contains(mode), now = new.flags.contains(mode)
            if now && !was { add.append(mode) }
            if was && !now && simple.contains(mode) { remove.append(mode) }
        }
        if new.key != (old.key ?? "") {
            if new.key.isEmpty {
                remove.append("k")
                args.append(old.key ?? "*")
            } else {
                // A new key replaces the old one.
                if old.key != nil {
                    remove.append("k")
                    args.append(old.key!)
                }
                add.append("k")
            }
        }
        if new.limit != (old.limit ?? "") {
            if new.limit.isEmpty { remove.append("l") } else if Int(new.limit) != nil { add.append("l") }
        }
        // Arguments go in the order of the letters: removals first.
        var addArgs: [String] = []
        if add.contains("k") { addArgs.append(new.key) }
        if add.contains("l") { addArgs.append(new.limit) }
        let modes = (remove.isEmpty ? "" : "-" + remove) + (add.isEmpty ? "" : "+" + add)
        guard !modes.isEmpty else { return nil }
        return (["MODE", channel, modes] + args + addArgs).joined(separator: " ")
    }
}
