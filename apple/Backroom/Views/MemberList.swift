import BackroomCore
import SwiftUI

/// The selected channel's members: collapsible rank sections, then everyone
/// else. A lazy stack, not a `List`: big channels have thousands of members
/// and only the visible rows should be built.
struct MemberList: View {
    @Environment(AppModel.self) private var model
    @Environment(Preferences.self) private var prefs
    @Environment(\.theme) private var theme
    let buffer: BufferModel
    @AppStorage("collapsedRanks") private var collapsedRanks = ""

    struct Rank: Identifiable {
        let symbol: Character
        let title: String
        var id: Character { symbol }
    }

    /// Names for the usual prefixes; others are shown by their symbol.
    static let titles: [Character: String] = [
        "~": "Owners", "&": "Admins", "@": "Operators", "%": "Half-operators", "+": "Voiced",
    ]
    /// Everyone without a membership prefix.
    static let online = Rank(symbol: "-", title: "Online")

    var body: some View {
        let groups = Self.group(buffer.members)
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2, pinnedViews: [.sectionHeaders]) {
                Text("\(buffer.members.count) members")
                    .font(.caption)
                    .foregroundStyle(theme.mutedColor)
                    .padding(.bottom, 4)
                ForEach(groups, id: \.rank.id) { group in
                    let collapsed = collapsedRanks.contains(group.rank.symbol)
                    Section {
                        if !collapsed {
                            ForEach(group.members, id: \.nick) { member in
                                MemberRow(
                                    member: member,
                                    color: NickColor.color(
                                        nick: member.nick, prefix: member.prefix, theme: theme,
                                        coloring: prefs.nickColoring)
                                ) {
                                    model.openQuery(network: buffer.id.network, nick: member.nick)
                                }
                            }
                        }
                    } header: {
                        Button {
                            toggle(group.rank.symbol)
                        } label: {
                            HStack(spacing: 4) {
                                Image(systemName: "chevron.right")
                                    .rotationEffect(.degrees(collapsed ? 0 : 90))
                                    .font(.caption2.bold())
                                Text("\(group.rank.title) · \(group.members.count)")
                                    .font(.caption.bold())
                                Spacer()
                            }
                            .foregroundStyle(theme.mutedColor)
                            .padding(.vertical, 3)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .background(theme.backgroundColor)
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
        }
        .background(theme.backgroundColor)
        .overlay {
            if buffer.members.isEmpty {
                ContentUnavailableView("No Members Yet", systemImage: "person.2")
            }
        }
    }

    private func toggle(_ symbol: Character) {
        if collapsedRanks.contains(symbol) {
            collapsedRanks.removeAll { $0 == symbol }
        } else {
            collapsedRanks.append(symbol)
        }
    }

    /// Members by highest prefix, then everyone else as "Online". The core
    /// sorts by the network's own PREFIX order, so groups follow it, whatever
    /// ranks the network has.
    static func group(_ members: [Member]) -> [(rank: Rank, members: [Member])] {
        var groups: [(rank: Rank, members: [Member])] = []
        var rest: [Member] = []
        for member in members {
            guard let top = member.prefix.first else {
                rest.append(member)
                continue
            }
            if let i = groups.firstIndex(where: { $0.rank.symbol == top }) {
                groups[i].members.append(member)
            } else {
                groups.append((rank: Rank(symbol: top, title: titles[top] ?? "\(top) members"), members: [member]))
            }
        }
        if !rest.isEmpty { groups.append((rank: online, members: rest)) }
        return groups
    }
}

struct MemberRow: View {
    let member: Member
    let color: Color
    /// Opens a private conversation with this member.
    let message: () -> Void

    var body: some View {
        Text(NickColor.label(member.nick, prefix: member.prefix))
            .foregroundStyle(color)
            .lineLimit(1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
            .onTapGesture(count: 2, perform: message)
            .contextMenu {
                Button("Send Message", action: message)
            }
    }
}
