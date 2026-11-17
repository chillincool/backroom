//! The chat area's panes: a tree of splits whose leaves each show one
//! buffer, and tabs that each hold such a layout. Plain values, saved as
//! JSON so the layout survives a restart.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::buffer::BufferId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: Uuid,
    pub buffer: Option<BufferId>,
}

impl Pane {
    fn new(buffer: Option<BufferId>) -> Self {
        Pane {
            id: Uuid::new_v4(),
            buffer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitAxis {
    /// Side by side.
    Horizontal,
    /// Stacked.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDirection {
    Left,
    Right,
    Up,
    Down,
}

impl PaneDirection {
    fn axis(self) -> SplitAxis {
        match self {
            PaneDirection::Left | PaneDirection::Right => SplitAxis::Horizontal,
            PaneDirection::Up | PaneDirection::Down => SplitAxis::Vertical,
        }
    }

    fn forward(self) -> bool {
        matches!(self, PaneDirection::Right | PaneDirection::Down)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub id: Uuid,
    pub axis: SplitAxis,
    pub children: Vec<PaneNode>,
    /// Each child's share of the space, summing to 1.
    pub fractions: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PaneNode {
    Pane(Pane),
    Split(Split),
}

impl PaneNode {
    pub fn id(&self) -> Uuid {
        match self {
            PaneNode::Pane(p) => p.id,
            PaneNode::Split(s) => s.id,
        }
    }

    pub fn panes(&self) -> Vec<&Pane> {
        match self {
            PaneNode::Pane(p) => vec![p],
            PaneNode::Split(s) => s.children.iter().flat_map(PaneNode::panes).collect(),
        }
    }

    fn contains(&self, id: Uuid) -> bool {
        self.panes().iter().any(|p| p.id == id)
    }

    fn update(&mut self, id: Uuid, change: &mut dyn FnMut(&mut Pane)) {
        match self {
            PaneNode::Pane(p) if p.id == id => change(p),
            PaneNode::Pane(_) => {}
            PaneNode::Split(s) => s.children.iter_mut().for_each(|c| c.update(id, change)),
        }
    }

    fn update_split(&mut self, id: Uuid, change: &mut dyn FnMut(&mut Split)) {
        if let PaneNode::Split(s) = self {
            if s.id == id {
                change(s);
            } else {
                s.children
                    .iter_mut()
                    .for_each(|c| c.update_split(id, change));
            }
        }
    }

    fn splitting(self, id: Uuid, axis: SplitAxis, new: Pane) -> PaneNode {
        match self {
            PaneNode::Pane(ref p) if p.id == id => PaneNode::Split(Split {
                id: Uuid::new_v4(),
                axis,
                children: vec![self, PaneNode::Pane(new)],
                fractions: vec![0.5, 0.5],
            }),
            PaneNode::Pane(_) => self,
            PaneNode::Split(mut s) => {
                let Some(i) = s.children.iter().position(|c| c.contains(id)) else {
                    return PaneNode::Split(s);
                };
                if s.axis == axis && matches!(s.children[i], PaneNode::Pane(_)) {
                    // Same direction: share this split's space instead of nesting.
                    let half = s.fractions[i] / 2.0;
                    s.fractions[i] = half;
                    s.children.insert(i + 1, PaneNode::Pane(new));
                    s.fractions.insert(i + 1, half);
                } else {
                    let child = s.children.remove(i);
                    s.children.insert(i, child.splitting(id, axis, new));
                }
                PaneNode::Split(s)
            }
        }
    }

    /// Without the pane; `None` if nothing is left. Splits left with one
    /// child collapse into it.
    fn removing(self, id: Uuid) -> Option<PaneNode> {
        match self {
            PaneNode::Pane(p) => (p.id != id).then_some(PaneNode::Pane(p)),
            PaneNode::Split(mut s) => {
                let Some(i) = s.children.iter().position(|c| c.contains(id)) else {
                    return Some(PaneNode::Split(s));
                };
                let child = s.children.remove(i);
                match child.removing(id) {
                    Some(rest) => s.children.insert(i, rest),
                    None => {
                        let freed = s.fractions.remove(i);
                        // The neighbours share the freed space.
                        let total: f64 = s.fractions.iter().sum();
                        let n = s.fractions.len() as f64;
                        for f in &mut s.fractions {
                            *f = if total > 0.0 {
                                *f + freed * *f / total
                            } else {
                                1.0 / n
                            };
                        }
                    }
                }
                if s.children.len() == 1 {
                    s.children.pop()
                } else {
                    Some(PaneNode::Split(s))
                }
            }
        }
    }

    /// The nearest pane beside `id` in a direction: through the innermost
    /// split along that axis where there's a sibling on that side.
    fn neighbour(&self, id: Uuid, direction: PaneDirection) -> Option<Uuid> {
        let PaneNode::Split(s) = self else {
            return None;
        };
        let i = s.children.iter().position(|c| c.contains(id))?;
        if let Some(inner) = s.children[i].neighbour(id, direction) {
            return Some(inner);
        }
        if s.axis != direction.axis() {
            return None;
        }
        let j = if direction.forward() {
            i + 1
        } else {
            i.checked_sub(1)?
        };
        let candidates = s.children.get(j)?.panes();
        let pick = if direction.forward() {
            candidates.first()
        } else {
            candidates.last()
        };
        pick.map(|p| p.id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneLayout {
    pub root: PaneNode,
    pub focused: Uuid,
}

impl Default for PaneLayout {
    fn default() -> Self {
        PaneLayout::new(None)
    }
}

impl PaneLayout {
    pub fn new(buffer: Option<BufferId>) -> Self {
        let pane = Pane::new(buffer);
        PaneLayout {
            focused: pane.id,
            root: PaneNode::Pane(pane),
        }
    }

    /// Every pane, in reading order (left to right, top to bottom).
    pub fn panes(&self) -> Vec<&Pane> {
        self.root.panes()
    }

    pub fn focused_pane(&self) -> Option<&Pane> {
        self.panes().into_iter().find(|p| p.id == self.focused)
    }

    /// Buffers shown somewhere.
    pub fn visible_buffers(&self) -> BTreeSet<BufferId> {
        self.panes()
            .into_iter()
            .filter_map(|p| p.buffer.clone())
            .collect()
    }

    pub fn pane(&self, id: Uuid) -> Option<&Pane> {
        self.panes().into_iter().find(|p| p.id == id)
    }

    pub fn show(&mut self, buffer: Option<BufferId>, pane: Uuid) {
        self.root.update(pane, &mut |p| p.buffer = buffer.clone());
    }

    /// Splits `pane`, putting a new pane showing `buffer` after it (right of
    /// it, or below it). Returns the new pane, which gets the focus.
    pub fn split(&mut self, pane: Uuid, axis: SplitAxis, buffer: Option<BufferId>) -> Uuid {
        let new = Pane::new(buffer);
        let id = new.id;
        let root = std::mem::replace(&mut self.root, PaneNode::Pane(Pane::new(None)));
        self.root = root.splitting(pane, axis, new);
        self.focused = id;
        id
    }

    /// Closes a pane (never the last one); its neighbour gets the focus.
    pub fn close(&mut self, pane: Uuid) {
        let order: Vec<Uuid> = self.panes().iter().map(|p| p.id).collect();
        let Some(index) = order.iter().position(|&id| id == pane) else {
            return;
        };
        if order.len() < 2 {
            return;
        }
        let root = std::mem::replace(&mut self.root, PaneNode::Pane(Pane::new(None)));
        self.root = root.clone().removing(pane).unwrap_or(root);
        if self.focused == pane {
            let remaining = self.panes();
            self.focused = remaining[index.min(remaining.len() - 1)].id;
        }
    }

    /// Back to one pane: the focused one.
    pub fn close_others(&mut self) {
        if let Some(pane) = self.focused_pane().cloned() {
            self.root = PaneNode::Pane(pane);
        }
    }

    /// Sets the sizes of a split's children (fractions summing to 1).
    pub fn resize(&mut self, split: Uuid, fractions: Vec<f64>) {
        self.root.update_split(split, &mut |s| {
            if s.fractions.len() == fractions.len() {
                s.fractions = fractions.clone();
            }
        });
    }

    /// The pane next to the focused one in a direction, if any.
    pub fn neighbour(&self, direction: PaneDirection) -> Option<Uuid> {
        self.root.neighbour(self.focused, direction)
    }
}

/// A tab: its own pane layout, and a name if the user gave it one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceTab {
    pub id: Uuid,
    pub name: Option<String>,
    pub layout: PaneLayout,
}

impl WorkspaceTab {
    pub fn new(layout: PaneLayout) -> Self {
        WorkspaceTab {
            id: Uuid::new_v4(),
            name: None,
            layout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(name: &str) -> Option<BufferId> {
        Some(BufferId::new("1", name))
    }

    #[test]
    fn split_close_and_neighbours() {
        let mut layout = PaneLayout::new(b("#a"));
        let first = layout.focused;
        let second = layout.split(first, SplitAxis::Horizontal, b("#b"));
        let third = layout.split(second, SplitAxis::Horizontal, b("#c"));
        // Same axis: one split with three children, not nested.
        let PaneNode::Split(s) = &layout.root else {
            panic!()
        };
        assert_eq!(s.children.len(), 3);
        assert_eq!(s.fractions, [0.5, 0.25, 0.25]);
        assert_eq!(layout.neighbour(PaneDirection::Left), Some(second));
        let below = layout.split(third, SplitAxis::Vertical, b("#d"));
        assert_eq!(layout.neighbour(PaneDirection::Up), Some(third));
        assert_eq!(layout.neighbour(PaneDirection::Left), Some(second));
        layout.close(below);
        assert_eq!(layout.focused, third);
        layout.close(second);
        let PaneNode::Split(s) = &layout.root else {
            panic!()
        };
        assert_eq!(s.children.len(), 2);
        assert!((s.fractions.iter().sum::<f64>() - 1.0).abs() < 1e-9);
        layout.close_others();
        assert_eq!(layout.panes().len(), 1);
        layout.close(third);
        assert_eq!(layout.panes().len(), 1, "never the last pane");
    }

    #[test]
    fn round_trips_through_json() {
        let mut layout = PaneLayout::new(b("#a"));
        layout.split(layout.focused, SplitAxis::Vertical, None);
        let tab = WorkspaceTab::new(layout);
        let json = serde_json::to_string(&tab).unwrap();
        assert_eq!(serde_json::from_str::<WorkspaceTab>(&json).unwrap(), tab);
    }
}
