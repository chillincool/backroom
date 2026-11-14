import BackroomCore
import SwiftUI
import Testing

@testable import Backroom

@MainActor @Suite struct MemberListTests {
    @Test func groupsByHighestRankThenOnline() {
        let groups = MemberList.group([
            Member(nick: "owner", prefix: "~@"), Member(nick: "op", prefix: "@+"),
            Member(nick: "voice", prefix: "+"), Member(nick: "plain", prefix: ""),
        ])
        #expect(groups.map(\.rank.title) == ["Owners", "Operators", "Voiced", "Online"])
        #expect(groups.map { $0.members.map(\.nick) } == [["owner"], ["op"], ["voice"], ["plain"]])
    }

    @Test func unusualRanksGetTheirOwnGroupInNetworkOrder() {
        let groups = MemberList.group([
            Member(nick: "creator", prefix: "!@"), Member(nick: "op", prefix: "@"),
            Member(nick: "plain", prefix: ""),
        ])
        #expect(groups.map(\.rank.title) == ["! members", "Operators", "Online"])
        #expect(NickColor.color(nick: "n", prefix: "!", theme: .defaultLight, coloring: .rank) == .indigo)
    }

    @Test func labelsUseTheHighestPrefix() {
        #expect(NickColor.label("alice", prefix: "@+") == "@alice")
        #expect(NickColor.label("bob", prefix: "") == "bob")
        #expect(NickColor.label("erin", prefix: nil) == "erin")
    }

    @Test func coloursFollowStatus() {
        #expect(NickColor.color(nick: "n", prefix: "@+", theme: .defaultLight, coloring: .rank) == .green)
        #expect(NickColor.color(nick: "n", prefix: "+", theme: .defaultLight, coloring: .rank) == .orange)
        #expect(NickColor.color(nick: "n", prefix: "", theme: .defaultLight, coloring: .rank) == .primary)
        #expect(NickColor.color(nick: "n", prefix: "%", theme: .defaultLight, coloring: .rank) == .teal)
        #expect(NickColor.color(nick: "n", prefix: nil, theme: .defaultLight, coloring: .rank) == .primary)
    }
}
