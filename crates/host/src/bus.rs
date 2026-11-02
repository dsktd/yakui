//! The space's message bus (PROTOCOL.md): subscriptions by topic, the last message of every
//! retained topic, a bounded delivery queue per pane, and the one place commands are
//! validated and applied. Retained topics persist as `<space>/bus.json`.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

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
use pane_api::protocol::Owners;
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

/// The most a retained value may take as JSON, so a pane cannot fill bus.json.
const MAX_RETAINED: usize = 256 * 1024;

/// The least time between two messages on one `drag/<id>`: 60 a second.
pub const STREAM_INTERVAL: Duration = Duration::from_micros(16_667);

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
    /// Let go of every object `pane` holds, when the client driving it goes.
    fn release(
        &self,
        pane: PaneId,
    );
    /// What `pane` holds on the bus, to put back with [`Bus::restore`] if replacing it fails.
    fn hold(
        &self,
        pane: PaneId,
    ) -> Held;
    /// Replace whatever `pane` holds now (a half-built replacement) with `held`.
    fn restore(
        &self,
        pane: PaneId,
        held: Held,
    );
    /// Everything queued, in arrival order per pane, after the held stream messages that are
    /// due.
    fn take_queued(&self) -> Vec<(PaneId, String, Message)>;
    /// When the earliest held stream message comes due, if any is held.
    fn next_due(&self) -> Option<Instant>;
    /// Record the kind of `pane`, whose prefix is the only private one it may write.
    fn set_kind(
        &self,
        pane: PaneId,
        kind: &str,
    );
    fn save(&self) -> Result<()>;
}

/// A pane's subscriptions, kind and queued messages, as [`Bus::hold`] saw them.
pub struct Held {
    pub(crate) topics: Vec<String>,
    kind: Option<String>,
    queue: VecDeque<(String, Message)>,
}

/// The in-process bus: one mutex, delivery by queue, persistence by file.
#[derive(Clone)]
pub struct Local(Arc<Mutex<LocalInner>>);
struct LocalInner {
    origin: HostId,
    /// Where this bus's `ts` count from.
    epoch: Instant,
    retained: BTreeMap<String, Message>,
    subscribers: BTreeMap<String, BTreeSet<PaneId>>,
    kinds: BTreeMap<PaneId, String>,
    queues: BTreeMap<PaneId, VecDeque<(String, Message)>>,
    throttles: BTreeMap<String, Throttle>,
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
        let entries: BTreeMap<String, Value> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(_) => BTreeMap::new(),
        };
        // An entry that is not a message (a hand edit) is dropped, not the whole space.
        let mut retained = BTreeMap::new();
        for (topic, entry) in entries {
            match serde_json::from_value::<Message>(entry) {
                Ok(msg) => {
                    retained.insert(topic, msg);
                }
                Err(err) => warn!("{}: dropped {topic}: {err}", path.display()),
            }
        }
        let mut inner = LocalInner {
            origin,
            epoch: Instant::now(),
            retained,
            subscribers: BTreeMap::new(),
            kinds: BTreeMap::new(),
            queues: BTreeMap::new(),
            throttles: BTreeMap::new(),
            path,
            dirty: false,
        };
        inner.publish(None, Topic::PROTOCOL, json!({ "version": VERSION }), true);
        // No pane holds anything across a restart.
        inner.publish(None, Topic::OWNERS, json!({ "owners": {} }), true);
        info!(
            "{} retained topics from {}",
            inner.retained.len(),
            inner.path.display()
        );
        Ok(Self(Arc::new(Mutex::new(inner))))
    }
    /// Retain [`seed_scene`] if the bus has no `scene`, so a fresh space shows something
    /// before its first subscriber gets its retained message.
    pub fn seed(&self) {
        let mut bus = self.0.lock().unwrap();
        if bus.retained.contains_key(Topic::SCENE) {
            return;
        }
        let _ = bus.retain_scene(&seed_scene());
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
        bus.kinds.remove(&pane);
        bus.release(pane);
    }
    fn release(
        &self,
        pane: PaneId,
    ) {
        self.0.lock().unwrap().release(pane);
    }
    fn hold(
        &self,
        pane: PaneId,
    ) -> Held {
        let bus = self.0.lock().unwrap();
        Held {
            topics: bus
                .subscribers
                .iter()
                .filter(|(_topic, panes)| panes.contains(&pane))
                .map(|(topic, _panes)| topic.clone())
                .collect(),
            kind: bus.kinds.get(&pane).cloned(),
            queue: bus.queues.get(&pane).cloned().unwrap_or_default(),
        }
    }
    fn restore(
        &self,
        pane: PaneId,
        held: Held,
    ) {
        self.unsubscribe(pane);
        let mut bus = self.0.lock().unwrap();
        for topic in held.topics {
            bus.subscribers.entry(topic).or_default().insert(pane);
        }
        if let Some(kind) = held.kind {
            bus.kinds.insert(pane, kind);
        }
        if !held.queue.is_empty() {
            bus.queues.insert(pane, held.queue);
        }
    }
    fn set_kind(
        &self,
        pane: PaneId,
        kind: &str,
    ) {
        self.0.lock().unwrap().kinds.insert(pane, kind.to_owned());
    }
    fn next_due(&self) -> Option<Instant> {
        self.0.lock().unwrap().next_due()
    }
    fn take_queued(&self) -> Vec<(PaneId, String, Message)> {
        let mut bus = self.0.lock().unwrap();
        bus.flush(Instant::now());
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
        let kind = bus.kinds.get(&pane).map(String::as_str);
        let checked = check_topic(kind, topic, retain).and_then(|()| match retain {
            true => check_size(&value),
            false => Ok(()),
        });
        if let Err(err) = checked {
            bus.refuse(pane, topic, &err);
            return;
        }
        // A drag from anyone but the holder is noise from a lost race, not worth an answer.
        if let Some(id) = Topic::drag_id(topic)
            && bus.owners().holder(id) != Some(pane)
        {
            debug!("pane {pane} does not hold {id}, dropped {topic}");
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
            ts: None,
        };
        if retain {
            let last = self.retained.get(topic);
            if last.is_some_and(|last| last.value == msg.value) {
                return;
            }
            // A Lamport count: one past the last message, whichever host wrote it, so hosts
            // sharing the topic agree on the newest by `(seq, origin)`.
            let seq = last.and_then(|last| last.seq).unwrap_or(0) + 1;
            msg.seq = Some(seq);
            self.retained.insert(topic.to_owned(), msg.clone());
            self.dirty = true;
        } else {
            let now = Instant::now();
            msg.ts = Some((now - self.epoch).as_millis() as u64);
            if !self.throttle(topic, &msg, now) {
                return;
            }
        }
        debug!("{from:?} -> {topic}");
        self.fan_out(topic, msg);
    }
    /// Whether a transient message may go now. A `drag/<id>` within [`STREAM_INTERVAL`] of
    /// the last one sent is held instead, replacing any held before it, until [`Self::flush`].
    fn throttle(
        &mut self,
        topic: &str,
        msg: &Message,
        now: Instant,
    ) -> bool {
        if Topic::drag_id(topic).is_none() {
            return true;
        }
        match self.throttles.get_mut(topic) {
            Some(throttle) if now < throttle.sent + STREAM_INTERVAL => {
                throttle.held = Some(msg.clone());
                false
            }
            _ => {
                let throttle = Throttle {
                    sent: now,
                    held: None,
                };
                self.throttles.insert(topic.to_owned(), throttle);
                true
            }
        }
    }
    fn next_due(&self) -> Option<Instant> {
        self.throttles
            .values()
            .filter(|throttle| throttle.held.is_some())
            .map(|throttle| throttle.sent + STREAM_INTERVAL)
            .min()
    }
    /// Send every held stream message whose interval is over; forget idle streams.
    fn flush(
        &mut self,
        now: Instant,
    ) {
        let mut due = Vec::new();
        self.throttles.retain(|topic, throttle| {
            if now < throttle.sent + STREAM_INTERVAL {
                return true;
            }
            let Some(msg) = throttle.held.take() else {
                return false;
            };
            throttle.sent = now;
            due.push((topic.clone(), msg));
            true
        });
        for (topic, msg) in due {
            self.fan_out(&topic, msg);
        }
    }
    /// Queue `msg` for every subscriber of `topic`.
    fn fan_out(
        &mut self,
        topic: &str,
        msg: Message,
    ) {
        let panes = match self.subscribers.get(topic) {
            Some(panes) => panes.iter().copied().collect::<Vec<_>>(),
            None => Vec::new(),
        };
        for pane in panes {
            self.enqueue(pane, topic, msg.clone());
        }
    }
    /// Drop a write of `pane` to `topic`: say why on `rejected/<pane>` and in the log.
    fn refuse(
        &mut self,
        pane: PaneId,
        topic: &str,
        err: &anyhow::Error,
    ) {
        warn!("pane {pane} may not write {topic:?}: {err}");
        let value = json!({ "topic": topic, "reason": err.to_string() });
        self.publish(None, &Topic::rejected(pane), value, false);
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
        let Err(err) = self.apply(pane, &cmd) else {
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
    ) -> Result<()> {
        self.retain_checked(Topic::SCENE, serde_json::to_value(scene).unwrap())
    }
    fn owners(&self) -> Owners {
        match self.retained.get(Topic::OWNERS) {
            Some(msg) => Owners::from_message(&msg.value),
            None => Owners::default(),
        }
    }
    fn retain_owners(
        &mut self,
        owners: &Owners,
    ) -> Result<()> {
        self.retain_checked(Topic::OWNERS, serde_json::to_value(owners).unwrap())?;
        // A held drag of an object nobody holds now would land after its final position.
        self.throttles.retain(|topic, _throttle| {
            Topic::drag_id(topic).is_some_and(|id| owners.holder(id).is_some())
        });
        Ok(())
    }
    /// Drop every object `pane` holds.
    fn release(
        &mut self,
        pane: PaneId,
    ) {
        let mut owners = self.owners();
        owners.owners.retain(|_id, holder| *holder != pane);
        let _ = self.retain_owners(&owners);
    }
    fn retain_selection(
        &mut self,
        selection: &Selection,
    ) -> Result<()> {
        self.retain_checked(Topic::SELECTION, serde_json::to_value(selection).unwrap())
    }
    /// Retain a standard topic's new value, unless it is too big to keep.
    fn retain_checked(
        &mut self,
        topic: &str,
        value: Value,
    ) -> Result<()> {
        check_size(&value)?;
        self.publish(None, topic, value, true);
        Ok(())
    }
    /// Validate first, then publish every topic the command touches, so a rejected command
    /// changes nothing.
    fn apply(
        &mut self,
        pane: PaneId,
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
                self.retain_selection(&selection)?;
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
                self.retain_scene(&scene)?;
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
                let mut owners = self.owners();
                owners.owners.retain(|id, _holder| !ids.contains(id));
                self.retain_scene(&scene)?;
                self.retain_selection(&selection)?;
                self.retain_owners(&owners)?;
            }
            Command::SetProperty { ids, key, value } => {
                if ids.is_empty() {
                    return Err(anyhow!("No objects to set"));
                }
                let mut scene = self.scene()?;
                for id in ids {
                    let object = scene
                        .objects
                        .iter_mut()
                        .find(|object| object.id == *id)
                        .ok_or(anyhow!("No object {id}"))?;
                    set_property(object, key, value)?;
                }
                self.retain_scene(&scene)?;
            }
            Command::MoveObject { id, position } => {
                self.check_holder(pane, id)?;
                let mut scene = self.scene()?;
                let object = scene
                    .objects
                    .iter_mut()
                    .find(|object| object.id == *id)
                    .ok_or(anyhow!("No object {id}"))?;
                check_vector(position)?;
                object.position = *position;
                self.retain_scene(&scene)?;
                self.throttles.remove(&Topic::drag(id));
            }
            Command::Grab { id } => {
                if self.scene()?.object(id).is_none() {
                    return Err(anyhow!("No object {id}"));
                }
                self.check_holder(pane, id)?;
                let mut owners = self.owners();
                owners.owners.insert(id.clone(), pane);
                self.retain_owners(&owners)?;
            }
            Command::Release { id } => {
                self.check_holder(pane, id)?;
                let mut owners = self.owners();
                owners.owners.remove(id);
                self.retain_owners(&owners)?;
            }
        }
        Ok(())
    }
    /// Whether `pane` may touch `id`: nobody holds it, or `pane` does.
    fn check_holder(
        &self,
        pane: PaneId,
        id: &str,
    ) -> Result<()> {
        match self.owners().holder(id) {
            Some(holder) if holder != pane => Err(anyhow!("Object {id} is held by {holder}")),
            _ => Ok(()),
        }
    }
}

/// One `drag/<id>`'s pace: when a message last went, and the latest one waiting since.
struct Throttle {
    sent: Instant,
    held: Option<Message>,
}

/// What a pane of `kind` may publish, or retain if `retain`: the standard transient topics
/// and `drag/<id>` (not retained), and `<kind>/<name>`.
fn check_topic(
    kind: Option<&str>,
    topic: &str,
    retain: bool,
) -> Result<()> {
    if Topic::TRANSIENT.contains(&topic) || Topic::drag_id(topic).is_some() {
        if retain {
            return Err(anyhow!("{topic} is transient, it cannot be retained"));
        }
        return Ok(());
    }
    let name = kind
        .and_then(|kind| topic.strip_prefix(kind))
        .and_then(|rest| rest.strip_prefix('/'));
    match name {
        Some(name) if !name.is_empty() => Ok(()),
        _ => Err(anyhow!("A pane writes only its own {kind:?}/<name> topics")),
    }
}

/// Four objects, one definition for every viewport.
fn seed_scene() -> Scene {
    let objects = [
        (
            "seed-box",
            "Box",
            "box",
            [-2.0, 0.0, 0.0],
            [1.5, 1.5, 1.5],
            "#e06c3a",
        ),
        (
            "seed-ball",
            "Ball",
            "sphere",
            [0.5, 0.5, 0.0],
            [1.0, 1.0, 1.0],
            "#3a8ee0",
        ),
        (
            "seed-slab",
            "Slab",
            "box",
            [0.0, -1.5, 0.0],
            [6.0, 0.3, 4.0],
            "#6b6b6b",
        ),
        (
            "seed-lamp",
            "Lamp",
            "light",
            [2.0, 2.0, 1.0],
            [0.4, 0.4, 0.4],
            "#ffe08a",
        ),
    ];
    let objects = objects
        .into_iter()
        .map(|(id, name, kind, position, size, color)| Object {
            id: id.to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            position,
            size,
            color: color.to_owned(),
            ..Default::default()
        })
        .collect();
    Scene { objects }
}

fn check_size(value: &Value) -> Result<()> {
    let size = value.to_string().len();
    if size > MAX_RETAINED {
        return Err(anyhow!(
            "A retained value takes {size} bytes, over {MAX_RETAINED}"
        ));
    }
    Ok(())
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
        assert!(queued[1].2.ts.is_none());
        assert!(queued[2].2.ts.is_some());
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
        bus.set_kind(pid(1), "view3d");
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
    fn test_check_topic() {
        let ok = |kind, topic, retain| check_topic(kind, topic, retain).is_ok();
        // Standard transient topics, not retained; private ones under the own prefix.
        assert!(ok(Some("notes"), "hover", false));
        assert!(ok(Some("notes"), "tool", false));
        assert!(ok(Some("notes"), "focus", false));
        assert!(ok(Some("notes"), "notes/text", true));
        assert!(ok(Some("notes"), "notes/text", false));
        assert!(ok(Some("notes"), "notes/a/b", true));
        assert!(!ok(Some("notes"), "hover", true));
        // Another pane's prefix, a prefix that is only a string prefix, a standard retained
        // topic, a rejection, empty and malformed names, a pane of no known kind.
        assert!(!ok(Some("notes"), "clock/text", true));
        assert!(!ok(Some("notes"), "notesy/text", true));
        assert!(!ok(Some("notes"), "scene", false));
        assert!(!ok(Some("notes"), "selection", true));
        assert!(!ok(Some("notes"), "protocol", false));
        assert!(!ok(Some("notes"), "rejected/1", false));
        assert!(!ok(Some("notes"), "", false));
        assert!(!ok(Some("notes"), "notes", true));
        assert!(!ok(Some("notes"), "notes/", true));
        assert!(!ok(Some("notes"), "/text", true));
        assert!(!ok(None, "notes/text", true));
    }

    #[test]
    fn test_refused_write() {
        let bus = open(&tmp());
        bus.set_kind(pid(1), "notes");
        bus.subscribe(pid(1), &Topic::rejected(pid(1)));
        bus.subscribe(pid(2), "clock/text");
        bus.publish(pid(1), "clock/text", json!("x"), true);
        let queued = bus.take_queued();
        assert_eq!(
            topics(&queued),
            vec![(1, "rejected/00000000000000000000000000000abc-1")]
        );
        assert_eq!(queued[0].2.value["topic"], json!("clock/text"));
        assert!(queued[0].2.value["reason"].is_string());
        assert_eq!(bus.retained("clock/text"), None);
    }

    #[test]
    fn test_check_size() {
        // A string is its length plus two quotes.
        assert!(check_size(&json!("x".repeat(MAX_RETAINED - 2))).is_ok());
        assert!(check_size(&json!("x".repeat(MAX_RETAINED - 1))).is_err());
    }

    #[test]
    fn test_oversized_writes() {
        let bus = open(&tmp());
        bus.set_kind(pid(1), "notes");
        let rejected = Topic::rejected(pid(1));
        bus.subscribe(pid(1), &rejected);
        let big = json!("x".repeat(MAX_RETAINED));
        bus.publish(pid(1), "notes/text", big.clone(), true);
        assert_eq!(bus.retained("notes/text"), None);
        // Transient messages are not kept, so size does not matter.
        bus.subscribe(pid(2), "notes/text");
        bus.publish(pid(1), "notes/text", big, false);
        let queued = bus.take_queued();
        assert_eq!(
            topics(&queued),
            vec![
                (1, "rejected/00000000000000000000000000000abc-1"),
                (2, "notes/text")
            ]
        );
        // A command whose scene would be too big is rejected whole.
        let mut fat = object("a", "#ff0000");
        fat.props
            .insert("blob".to_owned(), json!("x".repeat(MAX_RETAINED)));
        command(&bus, pid(1), Command::AddObject { object: fat });
        assert_eq!(bus.retained(Topic::SCENE), None);
        let queued = bus.take_queued();
        assert_eq!(
            topics(&queued),
            vec![(1, "rejected/00000000000000000000000000000abc-1")]
        );
        assert!(
            queued[0].2.value["reason"]
                .as_str()
                .unwrap()
                .contains("over")
        );
    }

    #[test]
    fn test_seed() {
        let path = tmp();
        let bus = open(&path);
        bus.seed();
        bus.subscribe(pid(2), Topic::SCENE);
        let queued = bus.take_queued();
        assert_eq!(topics(&queued), vec![(2, "scene")]);
        assert_eq!(Scene::from_message(&queued[0].2.value).objects.len(), 4);
        // A scene already there, even an empty one, is left alone.
        command(
            &bus,
            pid(1),
            Command::RemoveObjects {
                ids: seed_scene().objects.into_iter().map(|o| o.id).collect(),
            },
        );
        let scene = bus.retained(Topic::SCENE).unwrap();
        bus.seed();
        assert_eq!(bus.retained(Topic::SCENE), Some(scene));
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
                ids: vec!["a".to_owned()],
                key: "color".to_owned(),
                value: json!("#12345"),
            },
            Command::SetProperty {
                ids: vec!["a".to_owned()],
                key: "position".to_owned(),
                value: json!("banana"),
            },
            Command::SetProperty {
                ids: vec!["a".to_owned()],
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
                ids: vec!["a".to_owned()],
                key: "color".to_owned(),
                value: json!("#ABCDEF"),
            },
            Command::SetProperty {
                ids: vec!["a".to_owned()],
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

    fn grab(id: &str) -> Command {
        Command::Grab { id: id.to_owned() }
    }

    fn release(id: &str) -> Command {
        Command::Release { id: id.to_owned() }
    }

    fn holder(
        bus: &Local,
        id: &str,
    ) -> Option<PaneId> {
        Owners::from_message(&bus.retained(Topic::OWNERS).unwrap().value).holder(id)
    }

    #[test]
    fn test_grab_in_arrival_order() {
        let bus = open(&tmp());
        add(&bus, "a");
        bus.subscribe(pid(1), &Topic::rejected(pid(1)));
        bus.subscribe(pid(2), &Topic::rejected(pid(2)));
        bus.take_queued();
        // The first grab wins; the second is denied on the loser's rejected topic.
        command(&bus, pid(1), grab("a"));
        command(&bus, pid(2), grab("a"));
        assert_eq!(holder(&bus, "a"), Some(pid(1)));
        let queued = bus.take_queued();
        assert_eq!(
            topics(&queued),
            vec![(2, "rejected/00000000000000000000000000000abc-2")]
        );
        let denied: Rejected = serde_json::from_value(queued[0].2.value.clone()).unwrap();
        assert_eq!(denied.cmd, grab("a"));
        assert!(denied.reason.contains("held by"));
        // Grabbing again is fine for the holder; nobody else moves or releases it.
        command(&bus, pid(1), grab("a"));
        let moved = Command::MoveObject {
            id: "a".to_owned(),
            position: [5.0, 0.0, 0.0],
        };
        command(&bus, pid(2), moved.clone());
        command(&bus, pid(2), release("a"));
        assert_eq!(topics(&bus.take_queued()).len(), 2);
        assert_eq!(holder(&bus, "a"), Some(pid(1)));
        let scene = Scene::from_message(&bus.retained(Topic::SCENE).unwrap().value);
        assert_eq!(scene.object("a").unwrap().position, [0.0; 3]);
        // The holder moves and lets go; then the other pane may grab it.
        command(&bus, pid(1), moved);
        command(&bus, pid(1), release("a"));
        assert_eq!(holder(&bus, "a"), None);
        command(&bus, pid(2), grab("a"));
        assert_eq!(holder(&bus, "a"), Some(pid(2)));
        assert!(bus.take_queued().is_empty());
        // An unknown object cannot be grabbed; a removed one is let go.
        command(&bus, pid(1), grab("zz"));
        assert_eq!(topics(&bus.take_queued()).len(), 1);
        let remove = Command::RemoveObjects {
            ids: vec!["a".to_owned()],
        };
        command(&bus, pid(1), remove);
        assert_eq!(holder(&bus, "a"), None);
    }

    #[test]
    fn test_drag_only_from_holder() {
        let bus = open(&tmp());
        bus.set_kind(pid(1), "view2d");
        bus.set_kind(pid(2), "view2d");
        add(&bus, "a");
        let topic = Topic::drag("a");
        bus.subscribe(pid(3), &topic);
        bus.subscribe(pid(2), &Topic::rejected(pid(2)));
        let sample = json!({ "id": "a", "position": [1, 2, 0] });
        // Nobody holds it, then pane 1 does: only pane 1's drags go through, silently.
        bus.publish(pid(1), &topic, sample.clone(), false);
        command(&bus, pid(1), grab("a"));
        bus.take_queued();
        bus.publish(pid(2), &topic, sample.clone(), false);
        bus.publish(pid(1), &topic, sample.clone(), false);
        let queued = bus.take_queued();
        assert_eq!(topics(&queued), vec![(3, "drag/a")]);
        assert!(queued[0].2.ts.is_some());
        // A drag topic is never retained.
        bus.publish(pid(1), &topic, sample, true);
        assert_eq!(bus.retained(&topic), None);
    }

    #[test]
    fn test_release_when_pane_goes() {
        let bus = open(&tmp());
        add(&bus, "a");
        add(&bus, "b");
        command(&bus, pid(1), grab("a"));
        command(&bus, pid(1), grab("b"));
        command(&bus, pid(2), grab("b"));
        bus.release(pid(2));
        assert_eq!(holder(&bus, "b"), Some(pid(1)));
        bus.release(pid(1));
        assert_eq!(holder(&bus, "a"), None);
        assert_eq!(holder(&bus, "b"), None);
        command(&bus, pid(1), grab("a"));
        bus.unsubscribe(pid(1));
        assert_eq!(holder(&bus, "a"), None);
    }

    #[test]
    fn test_drag_throttle_keeps_latest() {
        let bus = open(&tmp());
        bus.subscribe(pid(2), "drag/a");
        let mut inner = bus.0.lock().unwrap();
        let msg = |x| Message {
            value: json!({ "id": "a", "position": [x, 0, 0] }),
            origin: HOST,
            seq: None,
            ts: None,
        };
        let t0 = Instant::now();
        let step = STREAM_INTERVAL / 4;
        assert!(inner.throttle("drag/a", &msg(1), t0));
        assert!(!inner.throttle("drag/a", &msg(2), t0 + step));
        assert!(!inner.throttle("drag/a", &msg(3), t0 + step * 2));
        // Each object has its own pace, and other topics none.
        assert!(inner.throttle("drag/b", &msg(9), t0 + step));
        assert!(inner.throttle("hover", &msg(9), t0 + step));
        assert!(inner.throttle("hover", &msg(9), t0 + step));
        assert_eq!(inner.next_due(), Some(t0 + STREAM_INTERVAL));
        inner.flush(t0 + step * 3);
        assert!(inner.queues.is_empty());
        // Once the interval is over, only the latest goes, and the pace counts from it.
        inner.flush(t0 + STREAM_INTERVAL);
        let queue = inner.queues.remove(&pid(2)).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].1, msg(3));
        assert_eq!(inner.next_due(), None);
        let t1 = t0 + STREAM_INTERVAL;
        assert!(!inner.throttle("drag/a", &msg(4), t1 + step));
        assert!(inner.throttle("drag/a", &msg(5), t1 + STREAM_INTERVAL));
        // A stream with nothing held is forgotten after its interval.
        inner.flush(t1 + STREAM_INTERVAL * 3);
        assert!(inner.throttles.is_empty());
    }

    #[test]
    fn test_drop_discards_held_drag() {
        let bus = open(&tmp());
        bus.set_kind(pid(1), "view2d");
        add(&bus, "a");
        command(&bus, pid(1), grab("a"));
        bus.subscribe(pid(2), "drag/a");
        bus.subscribe(pid(2), Topic::SCENE);
        bus.take_queued();
        let sample = |x| json!({ "id": "a", "position": [x, 0, 0] });
        bus.publish(pid(1), "drag/a", sample(1), false);
        bus.publish(pid(1), "drag/a", sample(2), false);
        let moved = Command::MoveObject {
            id: "a".to_owned(),
            position: [3.0, 0.0, 0.0],
        };
        command(&bus, pid(1), moved);
        assert_eq!(bus.next_due(), None);
        bus.0
            .lock()
            .unwrap()
            .flush(Instant::now() + STREAM_INTERVAL * 2);
        let queued = bus.take_queued();
        assert_eq!(topics(&queued), vec![(2, "drag/a"), (2, "scene")]);
        assert_eq!(queued[0].2.value, sample(1));
    }

    #[test]
    fn test_owners_reset_on_open() {
        let path = tmp();
        let bus = open(&path);
        add(&bus, "a");
        command(&bus, pid(1), grab("a"));
        bus.save().unwrap();
        assert_eq!(holder(&open(&path), "a"), None);
    }

    #[test]
    fn test_set_property_many() {
        let bus = open(&tmp());
        bus.subscribe(pid(2), Topic::SCENE);
        for id in ["a", "b", "c"] {
            add(&bus, id);
        }
        bus.take_queued();
        let set = |ids: &[&str]| Command::SetProperty {
            ids: ids.iter().map(|id| id.to_string()).collect(),
            key: "color".to_owned(),
            value: json!("#00ff00"),
        };
        // One command, one scene.
        command(&bus, pid(1), set(&["a", "b"]));
        assert_eq!(topics(&bus.take_queued()), vec![(2, "scene")]);
        let scene = Scene::from_message(&bus.retained(Topic::SCENE).unwrap().value);
        assert_eq!(scene.object("a").unwrap().color, "#00ff00");
        assert_eq!(scene.object("b").unwrap().color, "#00ff00");
        assert_ne!(scene.object("c").unwrap().color, "#00ff00");
        // An unknown id, or none at all, rejects the whole command.
        let before = bus.0.lock().unwrap().retained.clone();
        command(&bus, pid(1), set(&["c", "zz"]));
        command(&bus, pid(1), set(&[]));
        assert!(topics(&bus.take_queued()).is_empty());
        assert_eq!(bus.0.lock().unwrap().retained, before);
    }

    #[test]
    fn test_persist() {
        let path = tmp();
        let bus = open(&path);
        bus.set_kind(pid(1), "view2d");
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
        again.set_kind(pid(1), "view2d");
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
        // An entry that is not a message is dropped; the others load.
        std::fs::write(
            &path,
            format!(
                r#"{{ "scene": 5, "view2d/view": {{ "value": {{ "zoom": 4 }}, "origin": "{origin}", "seq": 2 }} }}"#
            ),
        )
        .unwrap();
        let partial = open(&path);
        assert_eq!(partial.retained(Topic::SCENE), None);
        assert_eq!(partial.retained("view2d/view").unwrap().seq, Some(2));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
