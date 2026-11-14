import Foundation

/// The chat area's panes: a tree of splits whose leaves each show one
/// buffer. Plain values, saved as JSON so the layout survives a restart.
struct PaneLayout: Codable, Equatable {
    var root: PaneNode
    var focused: Pane.ID

    init(buffer: BufferID? = nil) {
        let pane = Pane(buffer: buffer)
        root = .pane(pane)
        focused = pane.id
    }

    /// Every pane, in reading order (left to right, top to bottom).
    var panes: [Pane] { root.panes }

    var focusedPane: Pane? { panes.first { $0.id == focused } }

    /// Buffers shown somewhere.
    var visibleBuffers: Set<BufferID> { Set(panes.compactMap(\.buffer)) }

    func pane(_ id: Pane.ID) -> Pane? { panes.first { $0.id == id } }

    mutating func show(_ buffer: BufferID?, in pane: Pane.ID) {
        root.update(pane) { $0.buffer = buffer }
    }

    /// Splits `pane`, putting a new pane showing `buffer` after it (right of
    /// it, or below it). Returns the new pane, which gets the focus.
    @discardableResult
    mutating func split(_ pane: Pane.ID, axis: SplitAxis, buffer: BufferID?) -> Pane.ID {
        let new = Pane(buffer: buffer)
        root = root.splitting(pane, axis: axis, with: new)
        focused = new.id
        return new.id
    }

    /// Closes a pane (never the last one); its neighbour gets the focus.
    mutating func close(_ pane: Pane.ID) {
        let order = panes
        guard order.count > 1, let index = order.firstIndex(where: { $0.id == pane }) else { return }
        root = root.removing(pane) ?? root
        if focused == pane {
            let remaining = panes
            focused = remaining[min(index, remaining.count - 1)].id
        }
    }

    /// Back to one pane: the focused one.
    mutating func closeOthers() {
        guard let pane = focusedPane else { return }
        root = .pane(pane)
    }

    /// Sets the sizes of a split's children (fractions summing to 1).
    mutating func resize(split: UUID, fractions: [Double]) {
        root.updateSplit(split) { $0.fractions = fractions }
    }

    /// The pane next to the focused one in a direction, if any.
    func neighbour(_ direction: PaneDirection) -> Pane.ID? {
        root.neighbour(of: focused, direction)
    }
}

struct Pane: Codable, Equatable, Identifiable {
    var id = UUID()
    var buffer: BufferID?
}

enum SplitAxis: String, Codable {
    /// Side by side.
    case horizontal
    /// Stacked.
    case vertical
}

enum PaneDirection {
    case left, right, up, down

    var axis: SplitAxis { self == .left || self == .right ? .horizontal : .vertical }
    var forward: Bool { self == .right || self == .down }
}

struct Split: Codable, Equatable, Identifiable {
    var id = UUID()
    var axis: SplitAxis
    var children: [PaneNode]
    /// Each child's share of the space, summing to 1.
    var fractions: [Double]
}

indirect enum PaneNode: Codable, Equatable, Identifiable {
    case pane(Pane)
    case split(Split)

    var id: UUID {
        switch self {
        case .pane(let pane): pane.id
        case .split(let split): split.id
        }
    }

    var panes: [Pane] {
        switch self {
        case .pane(let pane): [pane]
        case .split(let split): split.children.flatMap(\.panes)
        }
    }

    func contains(_ id: Pane.ID) -> Bool { panes.contains { $0.id == id } }

    mutating func update(_ id: Pane.ID, _ change: (inout Pane) -> Void) {
        switch self {
        case .pane(var pane) where pane.id == id:
            change(&pane)
            self = .pane(pane)
        case .pane: break
        case .split(var split):
            for i in split.children.indices { split.children[i].update(id, change) }
            self = .split(split)
        }
    }

    mutating func updateSplit(_ id: UUID, _ change: (inout Split) -> Void) {
        guard case .split(var split) = self else { return }
        if split.id == id {
            change(&split)
        } else {
            for i in split.children.indices { split.children[i].updateSplit(id, change) }
        }
        self = .split(split)
    }

    func splitting(_ id: Pane.ID, axis: SplitAxis, with new: Pane) -> PaneNode {
        switch self {
        case .pane(let pane) where pane.id == id:
            return .split(Split(axis: axis, children: [self, .pane(new)], fractions: [0.5, 0.5]))
        case .pane:
            return self
        case .split(var split):
            guard let i = split.children.firstIndex(where: { $0.contains(id) }) else { return self }
            if split.axis == axis, case .pane = split.children[i] {
                // Same direction: share this split's space instead of nesting.
                let half = split.fractions[i] / 2
                split.fractions[i] = half
                split.children.insert(.pane(new), at: i + 1)
                split.fractions.insert(half, at: i + 1)
            } else {
                split.children[i] = split.children[i].splitting(id, axis: axis, with: new)
            }
            return .split(split)
        }
    }

    /// Without the pane; nil if nothing is left. Splits left with one child
    /// collapse into it.
    func removing(_ id: Pane.ID) -> PaneNode? {
        switch self {
        case .pane(let pane): return pane.id == id ? nil : self
        case .split(var split):
            guard let i = split.children.firstIndex(where: { $0.contains(id) }) else { return self }
            if let rest = split.children[i].removing(id) {
                split.children[i] = rest
            } else {
                let freed = split.fractions.remove(at: i)
                split.children.remove(at: i)
                // The neighbours share the freed space.
                let total = split.fractions.reduce(0, +)
                split.fractions = split.fractions.map { total > 0 ? $0 + freed * $0 / total : 1 / Double(split.fractions.count) }
            }
            return split.children.count == 1 ? split.children[0] : .split(split)
        }
    }

    /// The nearest pane beside `id` in a direction: through the innermost
    /// split along that axis where there's a sibling on that side.
    func neighbour(of id: Pane.ID, _ direction: PaneDirection) -> Pane.ID? {
        guard case .split(let split) = self,
            let i = split.children.firstIndex(where: { $0.contains(id) })
        else { return nil }
        if let inner = split.children[i].neighbour(of: id, direction) { return inner }
        guard split.axis == direction.axis else { return nil }
        let j = direction.forward ? i + 1 : i - 1
        guard split.children.indices.contains(j) else { return nil }
        let candidates = split.children[j].panes
        return (direction.forward ? candidates.first : candidates.last)?.id
    }
}
