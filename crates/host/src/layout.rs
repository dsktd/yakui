//! The split tree and the pane table behind it. Pure data; panes never see it.

use std::collections::BTreeMap;

use anyhow::anyhow;
use pane_api::PaneId;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::Result;

/// Direction of the dividing line: `Vertical` puts `a` left of `b`.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, PartialEq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, PartialEq)]
pub enum Side {
    A,
    B,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Node {
    Leaf {
        pane: PaneId,
    },
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}
impl Node {
    fn leaf(pane: PaneId) -> Self {
        Node::Leaf { pane }
    }
    fn find_leaf(
        &mut self,
        pane: PaneId,
    ) -> Option<&mut Node> {
        match self {
            Node::Leaf { pane: id } if *id == pane => Some(self),
            Node::Leaf { .. } => None,
            Node::Split { a, b, .. } => a.find_leaf(pane).or_else(|| b.find_leaf(pane)),
        }
    }
    fn leaves(
        &self,
        into: &mut Vec<PaneId>,
    ) {
        match self {
            Node::Leaf { pane } => into.push(*pane),
            Node::Split { a, b, .. } => {
                a.leaves(into);
                b.leaves(into);
            }
        }
    }
    fn is_leaf(
        &self,
        pane: PaneId,
    ) -> bool {
        matches!(self, Node::Leaf { pane: id } if *id == pane)
    }
    fn swap(
        &mut self,
        x: PaneId,
        y: PaneId,
    ) {
        match self {
            Node::Leaf { pane } if *pane == x => *pane = y,
            Node::Leaf { pane } if *pane == y => *pane = x,
            Node::Leaf { .. } => {}
            Node::Split { a, b, .. } => {
                a.swap(x, y);
                b.swap(x, y);
            }
        }
    }
    /// Remove the leaf `pane`; its sibling takes the parent's place.
    fn join(
        &mut self,
        pane: PaneId,
    ) -> bool {
        let Node::Split { a, b, .. } = self else {
            return false;
        };
        let keep = if a.is_leaf(pane) {
            b
        } else if b.is_leaf(pane) {
            a
        } else {
            return a.join(pane) || b.join(pane);
        };
        *self = std::mem::replace(&mut **keep, Node::leaf(pane));
        true
    }
    /// Push the position of every `axis` edge in this node, spanning `lo..hi`, into `into`.
    fn edges(
        &self,
        axis: Axis,
        lo: f32,
        hi: f32,
        into: &mut Vec<f32>,
    ) {
        let Node::Split {
            axis: own,
            ratio,
            a,
            b,
        } = self
        else {
            return;
        };
        if *own != axis {
            a.edges(axis, lo, hi, into);
            b.edges(axis, lo, hi, into);
            return;
        }
        let at = lo + (hi - lo) * ratio;
        into.push(at);
        a.edges(axis, lo, at, into);
        b.edges(axis, at, hi, into);
    }
    /// Move this node's span from `from` to `to` along `axis`, keeping every inner edge in place.
    fn rescale(
        &mut self,
        axis: Axis,
        from: (f32, f32),
        to: (f32, f32),
    ) {
        let Node::Split {
            axis: own,
            ratio,
            a,
            b,
        } = self
        else {
            return;
        };
        if *own != axis {
            a.rescale(axis, from, to);
            b.rescale(axis, from, to);
            return;
        }
        let at = from.0 + (from.1 - from.0) * *ratio;
        *ratio = (at - to.0) / (to.1 - to.0);
        a.rescale(axis, (from.0, at), (to.0, at));
        b.rescale(axis, (at, from.1), (at, to.1));
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub kind: String,
    pub config: Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Layout {
    pub root: Node,
    pub panes: BTreeMap<PaneId, Entry>,
}

/// The smallest share of its split a pane can be dragged to.
const MIN: f32 = 0.05;

/// The Blender set. Clients apply these first; the host confirms with the whole [`Layout`].
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Op {
    Split {
        pane: PaneId,
        axis: Axis,
        new_pane: PaneId,
        kind: String,
    },
    Join {
        pane: PaneId,
    },
    Swap {
        a: PaneId,
        b: PaneId,
    },
    Resize {
        path: Vec<Side>,
        ratio: f32,
    },
    Change {
        pane: PaneId,
        kind: String,
    },
}

impl Layout {
    pub fn single(
        pane: PaneId,
        kind: impl Into<String>,
    ) -> Self {
        let mut panes = BTreeMap::new();
        panes.insert(
            pane,
            Entry {
                kind: kind.into(),
                config: Value::Null,
            },
        );
        Self {
            root: Node::leaf(pane),
            panes,
        }
    }
    pub fn leaves(&self) -> Vec<PaneId> {
        let mut out = Vec::new();
        self.root.leaves(&mut out);
        out
    }
    pub fn split(
        &mut self,
        pane: PaneId,
        axis: Axis,
        new_pane: PaneId,
        kind: impl Into<String>,
    ) -> Result<()> {
        if self.panes.contains_key(&new_pane) {
            return Err(anyhow!("Pane {new_pane} already exists"));
        }
        let leaf = self.root.find_leaf(pane).ok_or(anyhow!("No pane {pane}"))?;
        *leaf = Node::Split {
            axis,
            ratio: 0.5,
            a: Box::new(Node::leaf(pane)),
            b: Box::new(Node::leaf(new_pane)),
        };
        self.panes.insert(
            new_pane,
            Entry {
                kind: kind.into(),
                config: Value::Null,
            },
        );
        Ok(())
    }
    pub fn join(
        &mut self,
        pane: PaneId,
    ) -> Result<()> {
        if !self.root.join(pane) {
            return Err(anyhow!("Pane {pane} has no sibling to join"));
        }
        self.panes.remove(&pane);
        Ok(())
    }
    pub fn swap(
        &mut self,
        a: PaneId,
        b: PaneId,
    ) -> Result<()> {
        if self.root.find_leaf(a).is_none() || self.root.find_leaf(b).is_none() {
            return Err(anyhow!("Cannot swap {a} and {b}"));
        }
        self.root.swap(a, b);
        Ok(())
    }
    pub fn resize(
        &mut self,
        path: &[Side],
        ratio: f32,
    ) -> Result<()> {
        let mut node = &mut self.root;
        for side in path {
            let Node::Split { a, b, .. } = node else {
                return Err(anyhow!("Path {path:?} leaves the tree"));
            };
            node = match side {
                Side::A => a,
                Side::B => b,
            };
        }
        let Node::Split {
            axis,
            ratio: old,
            a,
            b,
        } = node
        else {
            return Err(anyhow!("Path {path:?} ends on a leaf"));
        };
        // Only the panes on either side of this edge change: the edges nested in `a` and `b`
        // keep their place, so the new edge stops `MIN` short of the nearest one.
        let (mut inner_a, mut inner_b) = (Vec::new(), Vec::new());
        a.edges(*axis, 0.0, *old, &mut inner_a);
        b.edges(*axis, *old, 1.0, &mut inner_b);
        let low = inner_a.into_iter().fold(0.0, f32::max) + MIN;
        let high = inner_b.into_iter().fold(1.0, f32::min) - MIN;
        let new = ratio.clamp(low, high.max(low));
        a.rescale(*axis, (0.0, *old), (0.0, new));
        b.rescale(*axis, (*old, 1.0), (new, 1.0));
        *old = new;
        Ok(())
    }
    pub fn change(
        &mut self,
        pane: PaneId,
        kind: impl Into<String>,
    ) -> Result<()> {
        let entry = self.panes.get_mut(&pane).ok_or(anyhow!("No pane {pane}"))?;
        entry.kind = kind.into();
        entry.config = Value::Null;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use pane_api::HostId;

    use super::*;

    fn pid(local: u32) -> PaneId {
        PaneId::new(HostId(1), local)
    }

    fn two() -> Layout {
        let mut layout = Layout::single(pid(1), "clock");
        layout
            .split(pid(1), Axis::Vertical, pid(2), "notes")
            .unwrap();
        layout
    }

    #[test]
    fn test_split() {
        let layout = two();
        assert_eq!(layout.leaves(), vec![pid(1), pid(2)]);
        assert_eq!(layout.panes[&pid(2)].kind, "notes");
        assert!(two().split(pid(1), Axis::Vertical, pid(2), "x").is_err());
        assert!(two().split(pid(9), Axis::Vertical, pid(3), "x").is_err());
    }

    #[test]
    fn test_join() {
        let mut layout = two();
        layout
            .split(pid(2), Axis::Horizontal, pid(3), "clock")
            .unwrap();
        layout.join(pid(2)).unwrap();
        assert_eq!(layout.leaves(), vec![pid(1), pid(3)]);
        assert!(!layout.panes.contains_key(&pid(2)));
        layout.join(pid(1)).unwrap();
        assert_eq!(layout.root, Node::leaf(pid(3)));
        assert!(layout.join(pid(3)).is_err());
    }

    #[test]
    fn test_swap() {
        let mut layout = two();
        layout.swap(pid(1), pid(2)).unwrap();
        assert_eq!(layout.leaves(), vec![pid(2), pid(1)]);
        assert!(layout.swap(pid(1), pid(7)).is_err());
    }

    #[test]
    fn test_resize() {
        let mut layout = two();
        layout.resize(&[], 0.25).unwrap();
        let Node::Split { ratio, .. } = layout.root else {
            unreachable!()
        };
        assert_eq!(ratio, 0.25);
        assert!(layout.resize(&[Side::A], 0.5).is_err());
        layout.resize(&[], 2.0).unwrap();
        let Node::Split { ratio, .. } = layout.root else {
            unreachable!()
        };
        assert_eq!(ratio, 0.95);
    }

    /// Absolute right edges of the three leaves in `| 1 | 2 | 3 |`.
    fn edges(layout: &Layout) -> Vec<f32> {
        let mut out = Vec::new();
        layout.root.edges(Axis::Vertical, 0.0, 1.0, &mut out);
        out
    }

    fn three() -> Layout {
        let mut layout = two();
        layout
            .split(pid(2), Axis::Vertical, pid(3), "clock")
            .unwrap();
        layout.resize(&[], 0.3).unwrap();
        layout
    }

    #[test]
    fn test_resize_moves_one_edge() {
        let mut layout = three();
        let before = edges(&layout);
        layout.resize(&[], 0.4).unwrap();
        let after = edges(&layout);
        assert_eq!(after[0], 0.4);
        // The second edge stays, so the third pane keeps its width and the first two their sum.
        assert!((after[1] - before[1]).abs() < 1e-6);

        let before = edges(&layout);
        layout.resize(&[Side::B], 0.25).unwrap();
        let after = edges(&layout);
        assert_eq!(after[0], before[0]);
    }

    #[test]
    fn test_resize_clamps_at_neighbour() {
        let mut layout = three();
        let before = edges(&layout);
        layout.resize(&[], 0.99).unwrap();
        let after = edges(&layout);
        assert!((after[0] - (before[1] - MIN)).abs() < 1e-6);
        assert!((after[1] - before[1]).abs() < 1e-6);
    }
}
