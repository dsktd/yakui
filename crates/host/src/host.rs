//! Shared state: the layout, the live panes and the fan-out channel.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use log::debug;
use log::info;
use pane_api::Ctx;
use pane_api::PaneId;
use serde_json::Value;
use serde_json::json;
use tokio::sync::broadcast;

use crate::Result;
use crate::layout::Axis;
use crate::layout::Layout;
use crate::layout::Op;
use crate::registry::AnyPane;
use crate::registry::Registry;

/// One outgoing WebSocket text frame, already serialized. `pane` is `None` for layout frames.
#[derive(Clone, Debug)]
pub struct Frame {
    pub pane: Option<PaneId>,
    pub text: String,
}

#[derive(Clone)]
pub struct Host(Arc<Mutex<HostInner>>);
pub struct HostInner {
    layout: Layout,
    panes: HashMap<PaneId, Box<dyn AnyPane>>,
    registry: Registry,
    dir: PathBuf,
    tx: broadcast::Sender<Frame>,
}

impl Host {
    /// Load `<dir>/layout.json` or start with clock | notes, then create every pane.
    pub fn open(
        dir: impl Into<PathBuf>,
        registry: Registry,
    ) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("layout.json");
        let layout = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => {
                let mut layout = Layout::single(PaneId(1), "clock");
                layout.split(PaneId(1), Axis::Vertical, PaneId(2), "notes")?;
                layout
            }
        };
        let mut panes = HashMap::new();
        for (id, entry) in &layout.panes {
            let ctx = ctx(&dir, *id);
            panes.insert(*id, registry.make(&entry.kind, entry.config.clone(), ctx)?);
        }
        info!("{} panes from {}", panes.len(), path.display());
        let (tx, _rx) = broadcast::channel(256);
        let inner = HostInner {
            layout,
            panes,
            registry,
            dir,
            tx,
        };
        Ok(Self(Arc::new(Mutex::new(inner))))
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.0.lock().unwrap().tx.subscribe()
    }
    pub fn module(
        &self,
        kind: &str,
    ) -> Option<&'static str> {
        self.0.lock().unwrap().registry.module(kind)
    }
    pub fn layout_json(&self) -> Value {
        self.0.lock().unwrap().layout_json()
    }
    /// The layout frame and a snapshot frame per pane (or just `only`), for a joining client.
    pub fn snapshot(
        &self,
        only: Option<PaneId>,
    ) -> Vec<Frame> {
        let host = self.0.lock().unwrap();
        let mut frames = vec![host.layout_frame()];
        for (id, pane) in &host.panes {
            if only.is_some_and(|only| only != *id) {
                continue;
            }
            frames.push(pane_frame(*id, pane.snapshot()));
        }
        frames
    }
    /// Route a client message to its pane and fan out what the pane emits.
    pub fn update(
        &self,
        pane: PaneId,
        msg: Value,
    ) -> Result<()> {
        let mut host = self.0.lock().unwrap();
        let p = host.panes.get_mut(&pane).ok_or(anyhow!("No pane {pane}"))?;
        for event in p.update(msg)? {
            let _ = host.tx.send(pane_frame(pane, event));
        }
        Ok(())
    }
    /// Apply a layout op, broadcast the result, then save. On a bad op nothing changes.
    /// A pane the op created is snapshotted after the layout, so clients have mounted it.
    pub fn apply(
        &self,
        op: Op,
    ) -> Result<()> {
        let mut host = self.0.lock().unwrap();
        debug!("{op:?}");
        let created = match op {
            Op::Split {
                pane,
                axis,
                new_pane,
                kind,
            } => {
                let p = host.make(&kind, new_pane)?;
                host.layout.split(pane, axis, new_pane, kind)?;
                host.panes.insert(new_pane, p);
                Some(new_pane)
            }
            Op::Join { pane } => {
                host.layout.join(pane)?;
                host.panes.remove(&pane);
                None
            }
            Op::Swap { a, b } => {
                host.layout.swap(a, b)?;
                None
            }
            Op::Resize { path, ratio } => {
                host.layout.resize(&path, ratio)?;
                None
            }
            Op::Change { pane, kind } => {
                let p = host.make(&kind, pane)?;
                host.layout.change(pane, kind)?;
                host.panes.insert(pane, p);
                Some(pane)
            }
        };
        let _ = host.tx.send(host.layout_frame());
        if let Some(pane) = created {
            let _ = host.tx.send(pane_frame(pane, host.panes[&pane].snapshot()));
        }
        host.save()
    }
}

impl HostInner {
    fn make(
        &self,
        kind: &str,
        id: PaneId,
    ) -> Result<Box<dyn AnyPane>> {
        self.registry.make(kind, Value::Null, ctx(&self.dir, id))
    }
    fn save(&self) -> Result<()> {
        let path = self.dir.join("layout.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&self.layout)?)?;
        Ok(())
    }
    fn layout_json(&self) -> Value {
        json!({ "layout": self.layout, "kinds": self.registry.kinds() })
    }
    fn layout_frame(&self) -> Frame {
        Frame {
            pane: None,
            text: self.layout_json().to_string(),
        }
    }
}

fn ctx(
    dir: &Path,
    id: PaneId,
) -> Ctx {
    Ctx {
        id,
        storage: dir.join("panes").join(id.to_string()),
    }
}

fn pane_frame(
    pane: PaneId,
    payload: Value,
) -> Frame {
    let text = json!({ "pane_id": pane, "payload": payload }).to_string();
    Frame {
        pane: Some(pane),
        text,
    }
}
