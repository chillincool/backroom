import BackroomCore
import Foundation
import Testing

@testable import Backroom

@Suite struct TimelineTests {
    func m(_ sender: String, _ kind: MessageKind, at t: TimeInterval) -> ChatMessage {
        ChatMessage(id: "\(sender)\(t)", time: Date(timeIntervalSince1970: t), sender: sender,
                    kind: kind, text: "", own: false)
    }

    @Test func foldsRunsOfJoinsAndParts() {
        let items = TimelineItem.build([
            m("a", .join, at: 1), m("b", .join, at: 2), m("c", .part, at: 3),
            m("a", .privmsg, at: 4),
            m("d", .join, at: 5),
        ])
        #expect(items.count == 3)
        guard case .presence(let first) = items[0], case .message = items[1], case .presence(let last) = items[2]
        else { Issue.record("wrong shape: \(items)"); return }
        #expect(TimelineItem.summary(first) == "→ a and b joined · ← c left")
        #expect(TimelineItem.summary(last) == "→ d joined")
    }

    @Test func longListsAreShortened() {
        let run = (1...20).map { m("u\($0)", .join, at: TimeInterval($0)) }
        #expect(TimelineItem.summary(run) == "→ u1, u2, u3, u4 and 16 others joined")
        let five = (1...5).map { m("u\($0)", .join, at: TimeInterval($0)) }
        #expect(TimelineItem.summary(five) == "→ u1, u2, u3, u4 and u5 joined")
    }

    @Test func repeatsAreNamedOnce() {
        let run = [m("a", .join, at: 1), m("a", .part, at: 2), m("A", .join, at: 3)]
        #expect(TimelineItem.summary(run) == "→ a joined · ← a left")
    }
}
