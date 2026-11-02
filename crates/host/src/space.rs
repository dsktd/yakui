//! A space: one layout, its live panes and the fan-out channel, stored under one directory.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use log::debug;
use log::info;
use log::warn;
use pane_api::Bus as PaneBus;
use pane_api::Ctx;
use pane_api::HostId;
use pane_api::PaneId;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use tokio::sync::broadcast;

use crate::Result;
use crate::bus::Bus;
use crate::bus::Local;
use crate::layout::Axis;
use crate::layout::Layout;
use crate::layout::Op;
use crate::registry::AnyPane;
use crate::registry::Registry;

/// A random id the client made: 16 to 64 lowercase letters and digits, so it is a URL
/// segment and a directory name as is.
#[derive(Deserialize, Clone, Debug, Eq, PartialEq, Hash)]
#[serde(try_from = "String")]
pub struct SpaceId(String);
impl SpaceId {
    /// 128 random bits as hex, the same shape the shell makes.
    pub fn random() -> Self {
        Self(format!("{:032x}", rand::random::<u128>()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl FromStr for SpaceId {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        if !(16..=64).contains(&s.len()) {
            return Err(anyhow!("Space id must be 16 to 64 characters"));
        }
        if !s
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(anyhow!("Space id must be lowercase letters and digits"));
        }
        Ok(Self(s.to_owned()))
    }
}
impl TryFrom<String> for SpaceId {
    type Error = anyhow::Error;
    fn try_from(s: String) -> Result<Self> {
        s.parse()
    }
}
impl std::fmt::Display for SpaceId {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One outgoing WebSocket text frame, already serialized. `pane` is `None` for layout frames.
#[derive(Clone, Debug)]
pub struct Frame {
    pub pane: Option<PaneId>,
    pub text: String,
}

#[derive(Clone)]
pub struct Space(Arc<Mutex<SpaceInner>>);
pub struct SpaceInner {
    layout: Layout,
    panes: HashMap<PaneId, Box<dyn AnyPane>>,
    registry: Registry,
    bus: Arc<dyn Bus>,
    host: HostId,
    dir: PathBuf,
    tx: broadcast::Sender<Frame>,
    /// The client connection that last sent each pane a message.
    drivers: HashMap<PaneId, u64>,
}

impl Space {
    /// Load `<dir>/layout.json` or start with clock | notes, then create every pane. The bus
    /// is opened first, so a pane's `new()` can subscribe to it.
    pub fn open(
        dir: impl Into<PathBuf>,
        registry: Registry,
        host: HostId,
    ) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)?;
        let local = Local::open(dir.join("bus.json"), host)?;
        local.seed();
        let bus: Arc<dyn Bus> = Arc::new(local);
        let path = dir.join("layout.json");
        let layout = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => {
                let one = PaneId::new(host, 1);
                let two = PaneId::new(host, 2);
                let mut layout = Layout::single(one, "clock");
                layout.split(one, Axis::Vertical, two, "notes")?;
                layout
            }
        };
        let mut panes = HashMap::new();
        for (id, entry) in &layout.panes {
            let ctx = ctx(&dir, &bus, *id, &entry.kind);
            panes.insert(*id, registry.make(&entry.kind, entry.config.clone(), ctx)?);
        }
        info!("{} panes from {}", panes.len(), path.display());
        let (tx, _rx) = broadcast::channel(256);
        let mut inner = SpaceInner {
            layout,
            panes,
            registry,
            bus,
            host,
            dir,
            tx,
            drivers: HashMap::new(),
        };
        inner.settle()?;
        Ok(Self(Arc::new(Mutex::new(inner))))
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.0.lock().unwrap().tx.subscribe()
    }
    pub fn layout_json(&self) -> Value {
        self.0.lock().unwrap().layout_json()
    }
    /// The layout frame and a snapshot frame per pane (or just `only`), for a joining client.
    pub fn snapshot(
        &self,
        only: Option<PaneId>,
    ) -> Vec<Frame> {
        let space = self.0.lock().unwrap();
        let mut frames = vec![space.layout_frame()];
        for (id, pane) in &space.panes {
            if only.is_some_and(|only| only != *id) {
                continue;
            }
            frames.push(pane_frame(*id, pane.snapshot()));
        }
        frames
    }
    /// Route a message from client connection `conn` to its pane and fan out what the pane
    /// emits.
    pub fn update(
        &self,
        conn: u64,
        pane: PaneId,
        msg: Value,
    ) -> Result<()> {
        let mut space = self.0.lock().unwrap();
        space.drivers.insert(pane, conn);
        let p = space
            .panes
            .get_mut(&pane)
            .ok_or(anyhow!("No pane {pane}"))?;
        for event in p.update(msg)? {
            let _ = space.tx.send(pane_frame(pane, event));
        }
        space.settle()
    }
    /// Deliver what the bus held back (stream messages coming due) and fan out the result.
    pub fn tick(&self) -> Result<()> {
        self.0.lock().unwrap().settle()
    }
    /// Client connection `conn` is gone: every pane it was the last to drive lets go of what
    /// it holds, so a dropped client cannot keep an object grabbed.
    pub fn disconnect(
        &self,
        conn: u64,
    ) -> Result<()> {
        let mut space = self.0.lock().unwrap();
        let panes: Vec<PaneId> = space
            .drivers
            .iter()
            .filter(|(_pane, driver)| **driver == conn)
            .map(|(pane, _driver)| *pane)
            .collect();
        for pane in panes {
            space.drivers.remove(&pane);
            space.bus.release(pane);
        }
        space.settle()
    }
    /// Apply a layout op, broadcast the result, then save. On a bad op nothing changes.
    /// A pane the op created is snapshotted after the layout, so clients have mounted it.
    /// A pane the op dropped loses its bus subscriptions.
    pub fn apply(
        &self,
        op: Op,
    ) -> Result<()> {
        let mut space = self.0.lock().unwrap();
        debug!("{op:?}");
        let created = match op {
            Op::Split {
                pane,
                axis,
                new_pane,
                kind,
            } => {
                let p = space.make(&kind, new_pane)?;
                space.layout.split(pane, axis, new_pane, kind)?;
                space.panes.insert(new_pane, p);
                Some(new_pane)
            }
            Op::Join { pane } => {
                space.layout.join(pane)?;
                space.panes.remove(&pane);
                space.bus.unsubscribe(pane);
                None
            }
            Op::Swap { a, b } => {
                space.layout.swap(a, b)?;
                None
            }
            Op::Resize { path, ratio } => {
                space.layout.resize(&path, ratio)?;
                None
            }
            Op::Change { pane, kind } => {
                if !space.panes.contains_key(&pane) {
                    return Err(anyhow!("No pane {pane}"));
                }
                if !space.registry.kinds().contains(&kind.as_str()) {
                    return Err(anyhow!("Unknown pane kind {kind}"));
                }
                // The new pane subscribes under the same id, so the old one's subscriptions
                // go first; if anything fails they come back and the old pane stays.
                let held = space.bus.hold(pane);
                space.bus.unsubscribe(pane);
                let built = space
                    .make(&kind, pane)
                    .and_then(|p| space.layout.change(pane, kind).map(|()| p));
                match built {
                    Ok(p) => {
                        space.panes.insert(pane, p);
                        Some(pane)
                    }
                    Err(err) => {
                        space.bus.restore(pane, held);
                        return Err(err);
                    }
                }
            }
        };
        let _ = space.tx.send(space.layout_frame());
        if let Some(pane) = created {
            let _ = space
                .tx
                .send(pane_frame(pane, space.panes[&pane].snapshot()));
        }
        space.settle()?;
        space.save()
    }
}

impl SpaceInner {
    fn make(
        &self,
        kind: &str,
        id: PaneId,
    ) -> Result<Box<dyn AnyPane>> {
        self.registry
            .make(kind, Value::Null, ctx(&self.dir, &self.bus, id, kind))
    }
    /// Deliver what the last update put on the bus to the panes subscribed, fan out what
    /// they emit, then save the retained topics. A pane hearing a message may publish
    /// another, so this loops; a conversation that never settles is cut off.
    fn settle(&mut self) -> Result<()> {
        for _round in 0..16 {
            let queued = self.bus.take_queued();
            if queued.is_empty() {
                return self.bus.save();
            }
            for (id, topic, msg) in queued {
                let Some(pane) = self.panes.get_mut(&id) else {
                    continue;
                };
                let events = match pane.on_message(&topic, msg) {
                    Ok(events) => events,
                    Err(err) => {
                        warn!("pane {id} on {topic}: {err}");
                        continue;
                    }
                };
                for event in events {
                    let _ = self.tx.send(pane_frame(id, event));
                }
            }
        }
        warn!("bus still busy after 16 rounds, dropping the rest");
        self.bus.take_queued();
        self.bus.save()
    }
    fn save(&self) -> Result<()> {
        let path = self.dir.join("layout.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&self.layout)?)?;
        Ok(())
    }
    fn layout_json(&self) -> Value {
        json!({ "layout": self.layout, "kinds": self.registry.kinds(), "host": self.host })
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
    bus: &Arc<dyn Bus>,
    id: PaneId,
    kind: &str,
) -> Ctx {
    bus.set_kind(id, kind);
    Ctx {
        id,
        storage: dir.join("panes").join(id.to_string()),
        bus: PaneBus::new(id, bus.clone()),
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

#[cfg(test)]
mod test {
    use pane_api::protocol::Command;
    use pane_api::protocol::Owners;
    use pane_api::protocol::Topic;

    use super::*;

    #[test]
    fn test_space_id() {
        assert!("abcdefghijklmnop".parse::<SpaceId>().is_ok());
        assert!(
            "0123456789abcdef0123456789abcdef"
                .parse::<SpaceId>()
                .is_ok()
        );
        assert!("a".repeat(64).parse::<SpaceId>().is_ok());
        assert!("a".repeat(65).parse::<SpaceId>().is_err());
        assert!("abcdefghijklmno".parse::<SpaceId>().is_err());
        assert!("".parse::<SpaceId>().is_err());
        assert!("ABCDEFGHIJKLMNOP".parse::<SpaceId>().is_err());
        assert!("abcdefghijklmno-".parse::<SpaceId>().is_err());
        assert!("../../../../etc/x".parse::<SpaceId>().is_err());
        assert!("abcdefghijklmnöp".parse::<SpaceId>().is_err());
        assert!(SpaceId::random().as_str().parse::<SpaceId>().is_ok());
        assert_ne!(SpaceId::random(), SpaceId::random());
    }

    #[test]
    fn test_change_failure_keeps_old_pane() {
        let dir = std::env::temp_dir().join(format!("yakui-space-{:08x}", rand::random::<u32>()));
        let host = HostId(0xabc);
        let mut registry = Registry::default();
        registry.add::<pane_clock::Clock>();
        registry.add::<pane_inspector::Inspector>();
        registry.add::<pane_notes::Notes>();
        let space = Space::open(&dir, registry, host).unwrap();
        let one = PaneId::new(host, 1);
        let three = PaneId::new(host, 3);
        space
            .apply(Op::Split {
                pane: one,
                axis: Axis::Vertical,
                new_pane: three,
                kind: "inspector".to_owned(),
            })
            .unwrap();
        let topics_of = |pane| space.0.lock().unwrap().bus.hold(pane).topics;
        let before = topics_of(three);
        assert!(!before.is_empty());

        // A file where the new pane's storage dir must go: `make` fails after the checks.
        let storage = dir.join("panes").join(three.to_string());
        std::fs::remove_dir_all(&storage).unwrap();
        std::fs::write(&storage, b"").unwrap();
        let change = Op::Change {
            pane: three,
            kind: "clock".to_owned(),
        };
        assert!(space.apply(change).is_err());

        assert_eq!(topics_of(three), before);
        let inner = space.0.lock().unwrap();
        assert!(inner.panes.contains_key(&three));
        assert_eq!(inner.layout.panes[&three].kind, "inspector");
        drop(inner);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_disconnect_releases() {
        let dir = std::env::temp_dir().join(format!("yakui-space-{:08x}", rand::random::<u32>()));
        let host = HostId(0xabc);
        let mut registry = Registry::default();
        registry.add::<pane_clock::Clock>();
        registry.add::<pane_notes::Notes>();
        let space = Space::open(&dir, registry, host).unwrap();
        let one = PaneId::new(host, 1);
        let two = PaneId::new(host, 2);
        let holder = |id| {
            let bus = space.0.lock().unwrap().bus.clone();
            Owners::from_message(&bus.retained(Topic::OWNERS).unwrap().value).holder(id)
        };
        {
            let mut inner = space.0.lock().unwrap();
            for (pane, id) in [(one, "seed-box"), (two, "seed-ball")] {
                let grab = Command::Grab { id: id.to_owned() };
                inner
                    .bus
                    .publish(pane, Topic::CMD, serde_json::to_value(grab).unwrap(), false);
            }
            // Connection 7 drove pane 1 last, connection 8 pane 2.
            inner.drivers.insert(one, 7);
            inner.drivers.insert(two, 8);
        }
        space.disconnect(9).unwrap();
        assert_eq!(holder("seed-box"), Some(one));
        space.disconnect(7).unwrap();
        assert_eq!(holder("seed-box"), None);
        assert_eq!(holder("seed-ball"), Some(two));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
