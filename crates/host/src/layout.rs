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
        let Node::Split { ratio: r, .. } = node else {
            return Err(anyhow!("Path {path:?} ends on a leaf"));
        };
        *r = ratio.clamp(0.05, 0.95);
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
}
