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

    @Test func presenceCanBeShownOrHidden() {
        let messages = [m("a", .join, at: 1), m("b", .part, at: 2), m("a", .privmsg, at: 3)]
        #expect(TimelineItem.build(messages, presence: .show).count == 3)
        let hidden = TimelineItem.build(messages, presence: .hide)
        #expect(hidden.count == 1)
        guard case .message = hidden[0] else { Issue.record("\(hidden)"); return }
    }

    @Test func runsFromOneSenderAreContinued() {
        let items = TimelineItem.build([
            m("a", .privmsg, at: 1), m("A", .privmsg, at: 2), m("b", .privmsg, at: 3),
            m("b", .action, at: 4), m("b", .privmsg, at: 5),
            // A long pause breaks the run.
            m("b", .privmsg, at: 5 + TimelineItem.runGap),
        ])
        let continued = items.map { if case .message(_, let c) = $0 { c } else { false } }
        #expect(continued == [false, true, false, false, false, false])
    }

    @Test func daysAreSeparated() {
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let day: TimeInterval = 86_400
        let items = TimelineItem.build(
            [m("a", .privmsg, at: 10), m("a", .privmsg, at: day + 10), m("a", .privmsg, at: day + 20)],
            calendar: utc)
        #expect(items.count == 4)
        guard case .day(let date) = items[1], case .message(_, let continued) = items[2] else {
            Issue.record("\(items)"); return
        }
        #expect(date == Date(timeIntervalSince1970: day))
        #expect(!continued, "a new day starts a new run")
    }

    @Test func unreadLineGoesAboveTheFirstMessageFromOthers() {
        var own = m("me", .privmsg, at: 3)
        own.own = true
        let items = TimelineItem.build(
            [m("a", .privmsg, at: 1), m("b", .join, at: 3), own, m("a", .privmsg, at: 4), m("a", .privmsg, at: 5)],
            presence: .show, unreadAfter: Date(timeIntervalSince1970: 2))
        #expect(items.map(\.id) == ["a1.0", "b3.0", "me3.0", "unread-line", "a4.0", "a5.0"])
        // The line starts a new run.
        guard case .message(_, let continued) = items[4] else { Issue.record("\(items)"); return }
        #expect(!continued)
        #expect(!TimelineItem.build([m("a", .privmsg, at: 1)], unreadAfter: Date(timeIntervalSince1970: 1))
            .contains(.unreadLine))
    }

    @Test func quitsFoldWithPartsAndIgnoredPeopleAreHidden() {
        let items = TimelineItem.build(
            [m("a", .part, at: 1), m("b", .quit, at: 2), m("spam", .privmsg, at: 3), m("c", .privmsg, at: 4)],
            isIgnored: { $0 == "spam" })
        #expect(items.count == 2)
        guard case .presence(let run) = items[0] else { Issue.record("\(items)"); return }
        #expect(TimelineItem.summary(run) == "← a and b left")
    }
}
