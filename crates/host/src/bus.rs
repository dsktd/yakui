//! The space's message bus (PROTOCOL.md): subscriptions by topic, the last message of every
//! retained topic, a bounded delivery queue per pane, and the one place commands are
//! validated and applied. Retained topics persist as `<space>/bus.json`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use log::debug;
use log::info;
use log::warn;
use pane_api::BusStore;
use pane_api::HostId;
use pane_api::Message;
use pane_api::PaneId;
use pane_api::protocol::Command;
use pane_api::protocol::Object;
use pane_api::protocol::Rejected;
use pane_api::protocol::Scene;
use pane_api::protocol::Selection;
use pane_api::protocol::Topic;
use pane_api::protocol::VERSION;
use serde_json::Value;
use serde_json::json;

use crate::Result;

/// Messages waiting for one pane. Past this the bus drops the newest and logs it.
const QUEUE: usize = 256;

/// What a space needs of its bus, beyond what panes see. [`Local`] is the only
/// implementation; one that gossips with other hosts would be another.
pub trait Bus: BusStore {
    fn retained(
        &self,
        topic: &str,
    ) -> Option<Message>;
    /// Drop every subscription and queued message of `pane`, when the pane goes.
    fn unsubscribe(
        &self,
        pane: PaneId,
    );
    /// Everything queued, in arrival order per pane.
    fn take_queued(&self) -> Vec<(PaneId, String, Message)>;
    fn save(&self) -> Result<()>;
}

/// The in-process bus: one mutex, delivery by queue, persistence by file.
#[derive(Clone)]
pub struct Local(Arc<Mutex<LocalInner>>);
struct LocalInner {
    origin: HostId,
    retained: BTreeMap<String, Message>,
    subscribers: BTreeMap<String, BTreeSet<PaneId>>,
    queues: BTreeMap<PaneId, VecDeque<(String, Message)>>,
    path: PathBuf,
    dirty: bool,
}

impl Local {
    /// Load the retained topics from `path` if it is there, then retain `protocol`.
    pub fn open(
        path: impl Into<PathBuf>,
        origin: HostId,
    ) -> Result<Self> {
        let path = path.into();
        let retained = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => BTreeMap::new(),
        };
        let mut inner = LocalInner {
            origin,
            retained,
            subscribers: BTreeMap::new(),
            queues: BTreeMap::new(),
            path,
            dirty: false,
        };
        inner.publish(None, Topic::PROTOCOL, json!({ "version": VERSION }), true);
        info!(
            "{} retained topics from {}",
            inner.retained.len(),
            inner.path.display()
        );
        Ok(Self(Arc::new(Mutex::new(inner))))
    }
}

impl Bus for Local {
    fn retained(
        &self,
        topic: &str,
    ) -> Option<Message> {
        self.0.lock().unwrap().retained.get(topic).cloned()
    }
    fn unsubscribe(
        &self,
        pane: PaneId,
    ) {
        let mut bus = self.0.lock().unwrap();
        for panes in bus.subscribers.values_mut() {
            panes.remove(&pane);
        }
        bus.queues.remove(&pane);
    }
    fn take_queued(&self) -> Vec<(PaneId, String, Message)> {
        let mut bus = self.0.lock().unwrap();
        let mut out = Vec::new();
        for (pane, queue) in std::mem::take(&mut bus.queues) {
            for (topic, msg) in queue {
                out.push((pane, topic, msg));
            }
        }
        out
    }
    /// Write the file if a retained topic changed since the last save, through a temp file
    /// and a rename.
    fn save(&self) -> Result<()> {
        let mut bus = self.0.lock().unwrap();
        if !bus.dirty {
            return Ok(());
        }
        let tmp = bus.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&bus.retained)?)?;
        std::fs::rename(&tmp, &bus.path)?;
        bus.dirty = false;
        Ok(())
    }
}

impl BusStore for Local {
    fn subscribe(
        &self,
        pane: PaneId,
        topic: &str,
    ) {
        let mut bus = self.0.lock().unwrap();
        bus.subscribers
            .entry(topic.to_owned())
            .or_default()
            .insert(pane);
        if let Some(msg) = bus.retained.get(topic).cloned() {
            bus.enqueue(pane, topic, msg);
        }
    }
    fn publish(
        &self,
        pane: PaneId,
        topic: &str,
        value: Value,
        retain: bool,
    ) {
        let mut bus = self.0.lock().unwrap();
        if topic == Topic::CMD {
            bus.publish(Some(pane), topic, value.clone(), false);
            bus.command(pane, value);
            return;
        }
        if Topic::is_standard(topic) && (retain || Topic::RETAINED.contains(&topic)) {
            warn!("pane {pane} may not retain {topic}, dropped");
            return;
        }
        if topic.starts_with("rejected/") {
            warn!("pane {pane} may not publish {topic}, dropped");
            return;
        }
        bus.publish(Some(pane), topic, value, retain);
    }
}

impl LocalInner {
    fn enqueue(
        &mut self,
        pane: PaneId,
        topic: &str,
        msg: Message,
    ) {
        let queue = self.queues.entry(pane).or_default();
        if queue.len() >= QUEUE {
            warn!("pane {pane} queue full, dropped {topic}");
            return;
        }
        queue.push_back((topic.to_owned(), msg));
    }
    /// Keep the message if retained (a repeat of the last value changes nothing), then
    /// queue it for every subscriber.
    fn publish(
        &mut self,
        from: Option<PaneId>,
        topic: &str,
        value: Value,
        retain: bool,
    ) {
        let mut msg = Message {
            value,
            origin: self.origin,
            seq: None,
        };
        if retain {
            let last = self.retained.get(topic);
            if last.is_some_and(|last| last.value == msg.value) {
                return;
            }
            // Counts on from this host's own last message; another host's is its own count.
            let seq = match last {
                Some(last) if last.origin == self.origin => last.seq.unwrap_or(0) + 1,
                _ => 1,
            };
            msg.seq = Some(seq);
            self.retained.insert(topic.to_owned(), msg.clone());
            self.dirty = true;
        }
        let panes = match self.subscribers.get(topic) {
            Some(panes) => panes.iter().copied().collect::<Vec<_>>(),
            None => Vec::new(),
        };
        debug!("{from:?} -> {topic} ({} subscribers)", panes.len());
        for pane in panes {
            self.enqueue(pane, topic, msg.clone());
        }
    }
    /// Apply a command from `pane`, or answer it on `rejected/<pane>`.
    fn command(
        &mut self,
        pane: PaneId,
        value: Value,
    ) {
        let cmd: Command = match serde_json::from_value(value) {
            Ok(cmd) => cmd,
            Err(err) => {
                warn!("pane {pane} sent a bad command: {err}");
                return;
            }
        };
        let Err(err) = self.apply(&cmd) else {
            return;
        };
        warn!("pane {pane} {cmd:?}: {err}");
        let rejected = Rejected {
            cmd,
            reason: err.to_string(),
        };
        let topic = Topic::rejected(pane);
        self.publish(None, &topic, serde_json::to_value(rejected).unwrap(), false);
    }
    /// The `scene` as it must be for a command to touch it: missing is empty, malformed is
    /// an error.
    fn scene(&self) -> Result<Scene> {
        match self.retained.get(Topic::SCENE) {
            None => Ok(Scene::default()),
            Some(msg) => serde_json::from_value(msg.value.clone())
                .or(Err(anyhow!("The scene topic is malformed"))),
        }
    }
    fn retain_scene(
        &mut self,
        scene: &Scene,
    ) {
        self.publish(
            None,
            Topic::SCENE,
            serde_json::to_value(scene).unwrap(),
            true,
        );
    }
    fn retain_selection(
        &mut self,
        selection: &Selection,
    ) {
        self.publish(
            None,
            Topic::SELECTION,
            serde_json::to_value(selection).unwrap(),
            true,
        );
    }
    /// Validate first, then publish every topic the command touches, so a rejected command
    /// changes nothing.
    fn apply(
        &mut self,
        cmd: &Command,
    ) -> Result<()> {
        match cmd {
            Command::Select { ids } => {
                let scene = self.scene()?;
                let mut selection = Selection::default();
                for id in ids {
                    if scene.object(id).is_none() {
                        return Err(anyhow!("No object {id}"));
                    }
                    if !selection.ids.contains(id) {
                        selection.ids.push(id.clone());
                    }
                }
                self.retain_selection(&selection);
            }
            Command::AddObject { object } => {
                let mut scene = self.scene()?;
                let mut object = object.clone();
                if object.id.is_empty() {
                    object.id = fresh_id(&scene);
                } else if scene.object(&object.id).is_some() {
                    return Err(anyhow!("Object {} already exists", object.id));
                }
                check_object(&object)?;
                scene.objects.push(object);
                self.retain_scene(&scene);
            }
            Command::RemoveObjects { ids } => {
                let mut scene = self.scene()?;
                for id in ids {
                    if scene.object(id).is_none() {
                        return Err(anyhow!("No object {id}"));
                    }
                }
                scene.objects.retain(|object| !ids.contains(&object.id));
                let mut selection = match self.retained.get(Topic::SELECTION) {
                    Some(msg) => Selection::from_message(&msg.value),
                    None => Selection::default(),
                };
                selection.ids.retain(|id| !ids.contains(id));
                self.retain_scene(&scene);
                self.retain_selection(&selection);
            }
            Command::SetProperty { id, key, value } => {
                let mut scene = self.scene()?;
                let object = scene
                    .objects
                    .iter_mut()
                    .find(|object| object.id == *id)
                    .ok_or(anyhow!("No object {id}"))?;
                set_property(object, key, value)?;
                self.retain_scene(&scene);
            }
            Command::MoveObject { id, position } => {
                let mut scene = self.scene()?;
                let object = scene
                    .objects
                    .iter_mut()
                    .find(|object| object.id == *id)
                    .ok_or(anyhow!("No object {id}"))?;
                check_vector(position)?;
                object.position = *position;
                self.retain_scene(&scene);
            }
        }
        Ok(())
    }
}

fn fresh_id(scene: &Scene) -> String {
    loop {
        let id = format!("{:08x}", rand::random::<u32>());
        if scene.object(&id).is_none() {
            return id;
        }
    }
}

fn check_object(object: &Object) -> Result<()> {
    check_vector(&object.position)?;
    check_vector(&object.size)?;
    check_color(&object.color)
}

fn check_vector(v: &[f64; 3]) -> Result<()> {
    if v.iter().all(|x| x.is_finite()) {
        return Ok(());
    }
    Err(anyhow!("Vector {v:?} is not finite"))
}

/// `#rrggbb`, either case.
fn check_color(color: &str) -> Result<()> {
    let ok = color.len() == 7
        && color.starts_with('#')
        && color.bytes().skip(1).all(|b| b.is_ascii_hexdigit());
    if ok {
        return Ok(());
    }
    Err(anyhow!("Colour {color:?} is not #rrggbb"))
}

fn set_property(
    object: &mut Object,
    key: &str,
    value: &Value,
) -> Result<()> {
    let string = || {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or(anyhow!("{key} must be a string"))
    };
    let vector = || {
        serde_json::from_value::<[f64; 3]>(value.clone())
            .or(Err(anyhow!("{key} must be three numbers")))
    };
    match key {
        "id" | "props" => return Err(anyhow!("{key} cannot be set")),
        "name" => object.name = string()?,
        "kind" => object.kind = string()?,
        "position" => object.position = vector()?,
        "size" => object.size = vector()?,
        "color" => {
            let color = string()?;
            check_color(&color)?;
            object.color = color;
        }
        _ => {
            object.props.insert(key.to_owned(), value.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    const HOST: HostId = HostId(0xabc);

    fn pid(local: u32) -> PaneId {
        PaneId::new(HOST, local)
    }

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yakui-bus-{:08x}", rand::random::<u32>()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("bus.json")
    }

    fn open(path: &PathBuf) -> Local {
        Local::open(path, HOST).unwrap()
    }

    fn object(
        id: &str,
        color: &str,
    ) -> Object {
        Object {
            id: id.to_owned(),
            name: id.to_uppercase(),
            kind: "box".to_owned(),
            color: color.to_owned(),
            ..Default::default()
        }
    }

    fn command(
        bus: &Local,
        pane: PaneId,
        cmd: Command,
    ) {
        bus.publish(pane, Topic::CMD, serde_json::to_value(cmd).unwrap(), false);
    }

    fn add(
        bus: &Local,
        id: &str,
    ) {
        let cmd = Command::AddObject {
            object: object(id, "#ff0000"),
        };
        command(bus, pid(1), cmd);
    }

    fn select(ids: &[&str]) -> Command {
        Command::Select {
            ids: ids.iter().map(|id| id.to_string()).collect(),
        }
    }

    fn topics(queued: &[(PaneId, String, Message)]) -> Vec<(u32, &str)> {
        queued
            .iter()
            .map(|(pane, topic, _msg)| (pane.local, topic.as_str()))
            .collect()
    }

    #[test]
    fn test_subscribe_retained_first() {
        let bus = open(&tmp());
        add(&bus, "a");
        bus.subscribe(pid(2), Topic::SCENE);
        bus.subscribe(pid(2), Topic::PROTOCOL);
        bus.subscribe(pid(2), Topic::HOVER);
        bus.publish(pid(1), Topic::HOVER, json!({ "id": "a" }), false);
        let queued = bus.take_queued();
        assert_eq!(
            topics(&queued),
            vec![(2, "scene"), (2, "protocol"), (2, "hover")]
        );
        assert_eq!(queued[1].2.value, json!({ "version": 1 }));
        assert_eq!(queued[1].2.seq, Some(1));
        assert_eq!(queued[2].2.seq, None);
        assert!(queued.iter().all(|(_pane, _topic, msg)| msg.origin == HOST));
        assert!(bus.take_queued().is_empty());
        // Transient messages reach only current subscribers; the sender hears its own.
        bus.subscribe(pid(1), Topic::HOVER);
        bus.publish(pid(1), Topic::HOVER, json!({ "id": null }), false);
        assert_eq!(topics(&bus.take_queued()), vec![(1, "hover"), (2, "hover")]);
        bus.unsubscribe(pid(2));
        bus.publish(pid(1), Topic::HOVER, json!({ "id": "a" }), false);
        assert_eq!(topics(&bus.take_queued()), vec![(1, "hover")]);
    }

    #[test]
    fn test_private_and_standard_topics() {
        let bus = open(&tmp());
        bus.subscribe(pid(2), "view3d/camera");
        bus.publish(pid(1), "view3d/camera", json!({ "yaw": 1 }), true);
        bus.publish(pid(1), "view3d/camera", json!({ "yaw": 1 }), true);
        bus.publish(pid(1), "view3d/camera", json!({ "yaw": 2 }), true);
        bus.subscribe(pid(3), "view3d/camera");
        let queued = bus.take_queued();
        assert_eq!(
            topics(&queued),
            vec![
                (2, "view3d/camera"),
                (2, "view3d/camera"),
                (3, "view3d/camera")
            ]
        );
        assert_eq!(queued[0].2.seq, Some(1));
        assert_eq!(queued[1].2.seq, Some(2));
        assert_eq!(queued[2].2.seq, Some(2));
        assert_eq!(
            bus.retained("view3d/camera").unwrap().value,
            json!({ "yaw": 2 })
        );
        // Standard retained topics and rejections are the host's alone.
        bus.subscribe(pid(2), Topic::SCENE);
        bus.subscribe(pid(2), "rejected/2");
        bus.take_queued();
        bus.publish(pid(1), Topic::SCENE, json!("nope"), false);
        bus.publish(pid(1), Topic::SCENE, json!("nope"), true);
        bus.publish(pid(1), Topic::HOVER, json!({ "id": null }), true);
        bus.publish(pid(1), "rejected/2", json!("nope"), false);
        assert!(bus.take_queued().is_empty());
        assert_eq!(bus.retained(Topic::SCENE), None);
    }

    #[test]
    fn test_queue_bounded() {
        let bus = open(&tmp());
        bus.subscribe(pid(2), Topic::HOVER);
        for i in 0..(QUEUE + 10) {
            bus.publish(pid(1), Topic::HOVER, json!({ "id": i }), false);
        }
        assert_eq!(bus.take_queued().len(), QUEUE);
    }

    #[test]
    fn test_commands() {
        let bus = open(&tmp());
        let rejected = Topic::rejected(pid(1));
        bus.subscribe(pid(1), &rejected);
        bus.subscribe(pid(2), Topic::SCENE);
        bus.subscribe(pid(2), Topic::SELECTION);
        bus.subscribe(pid(2), Topic::CMD);
        add(&bus, "a");
        command(&bus, pid(1), select(&["a", "a"]));
        let queued = bus.take_queued();
        assert_eq!(
            topics(&queued),
            vec![(2, "cmd"), (2, "scene"), (2, "cmd"), (2, "selection")]
        );
        assert_eq!(queued[3].2.value, json!({ "ids": ["a"] }));
        // Repeating the selection changes nothing but the cmd echo.
        command(&bus, pid(1), select(&["a"]));
        assert_eq!(topics(&bus.take_queued()), vec![(2, "cmd")]);
        let before = bus.0.lock().unwrap().retained.clone();
        let bad = [
            select(&["zz"]),
            Command::AddObject {
                object: object("a", "#00ff00"),
            },
            Command::AddObject {
                object: object("b", "green"),
            },
            Command::RemoveObjects {
                ids: vec!["zz".to_owned()],
            },
            Command::SetProperty {
                id: "a".to_owned(),
                key: "color".to_owned(),
                value: json!("#12345"),
            },
            Command::SetProperty {
                id: "a".to_owned(),
                key: "position".to_owned(),
                value: json!("banana"),
            },
            Command::SetProperty {
                id: "a".to_owned(),
                key: "id".to_owned(),
                value: json!("b"),
            },
            Command::MoveObject {
                id: "zz".to_owned(),
                position: [0.0; 3],
            },
        ];
        let count = bad.len();
        for cmd in bad {
            command(&bus, pid(1), cmd);
        }
        let queued = bus.take_queued();
        let rejections = queued
            .iter()
            .filter(|(pane, topic, _msg)| *pane == pid(1) && *topic == rejected)
            .count();
        assert_eq!(rejections, count);
        assert!(queued.iter().all(|(_pane, topic, _msg)| topic != "scene"));
        assert_eq!(bus.0.lock().unwrap().retained, before);
        let good = [
            Command::SetProperty {
                id: "a".to_owned(),
                key: "color".to_owned(),
                value: json!("#ABCDEF"),
            },
            Command::SetProperty {
                id: "a".to_owned(),
                key: "weight".to_owned(),
                value: json!(3),
            },
            Command::MoveObject {
                id: "a".to_owned(),
                position: [1.0, 2.0, 3.0],
            },
            Command::AddObject {
                object: Object::default(),
            },
        ];
        for cmd in good {
            command(&bus, pid(1), cmd);
        }
        let scene = Scene::from_message(&bus.retained(Topic::SCENE).unwrap().value);
        assert_eq!(scene.objects.len(), 2);
        let a = scene.object("a").unwrap();
        assert_eq!(a.color, "#ABCDEF");
        assert_eq!(a.props["weight"], json!(3));
        assert_eq!(a.position, [1.0, 2.0, 3.0]);
        assert_eq!(scene.objects[1].id.len(), 8);
        // Removing drops the selection too.
        command(
            &bus,
            pid(1),
            Command::RemoveObjects {
                ids: vec!["a".to_owned()],
            },
        );
        assert_eq!(
            bus.retained(Topic::SELECTION).unwrap().value,
            json!({ "ids": [] })
        );
    }

    #[test]
    fn test_persist() {
        let path = tmp();
        let bus = open(&path);
        add(&bus, "a");
        bus.publish(pid(1), Topic::HOVER, json!({ "id": "a" }), false);
        bus.publish(pid(1), "view2d/view", json!({ "zoom": 2 }), true);
        bus.save().unwrap();
        let again = open(&path);
        assert_eq!(again.retained(Topic::SCENE), bus.retained(Topic::SCENE));
        assert_eq!(
            again.retained(Topic::PROTOCOL).unwrap().value,
            json!({ "version": 1 })
        );
        let view = again.retained("view2d/view").unwrap();
        assert_eq!(view.value, json!({ "zoom": 2 }));
        assert_eq!(view.seq, Some(1));
        assert_eq!(view.origin, HOST);
        assert_eq!(again.retained(Topic::HOVER), None);
        // The count goes on from the file.
        again.publish(pid(1), "view2d/view", json!({ "zoom": 3 }), true);
        assert_eq!(again.retained("view2d/view").unwrap().seq, Some(2));
        // Nothing dirty: saving again leaves the file alone.
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        bus.save().unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
        // A malformed scene in the file is kept as is and refuses commands.
        let origin = HOST.to_string();
        std::fs::write(
            &path,
            format!(r#"{{ "scene": {{ "value": "banana", "origin": "{origin}", "seq": 9 }} }}"#),
        )
        .unwrap();
        let broken = open(&path);
        let rejected = Topic::rejected(pid(1));
        broken.subscribe(pid(1), &rejected);
        command(&broken, pid(1), select(&["a"]));
        let queued = broken.take_queued();
        assert_eq!(
            topics(&queued),
            vec![(1, "rejected/00000000000000000000000000000abc-1")]
        );
        let reason = queued[0].2.value["reason"].as_str().unwrap();
        assert!(reason.contains("malformed"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
