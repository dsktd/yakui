//! A front view (x right, y up) of the `scene` on a canvas: pan, zoom, drag, click to select.
//! A drag grabs the object, streams it on `drag/<id>` and drops it with `move_object`.

use log::warn;
use pane_api::Bus;
use pane_api::Ctx;
use pane_api::HostId;
use pane_api::Message;
use pane_api::Out;
use pane_api::Pane;
use pane_api::protocol::Command;
use pane_api::protocol::Drag;
use pane_api::protocol::Focus;
use pane_api::protocol::Hover;
use pane_api::protocol::Owners;
use pane_api::protocol::Protocol;
use pane_api::protocol::Rejected;
use pane_api::protocol::Scene;
use pane_api::protocol::Selection;
use pane_api::protocol::Topic;
use serde::Deserialize;
use serde::Serialize;

/// The pan and zoom at rest, shared by every view2d of the space and kept across reloads.
const VIEW: &str = "view2d/view";

/// The pan and zoom: the centre in world units, and pixels per unit.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct View {
    x: f64,
    y: f64,
    zoom: f64,
}
impl View {
    fn is_valid(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.zoom.is_finite() && self.zoom > 0.0
    }
}

#[derive(Serialize, Deserialize)]
pub enum Msg {
    /// A click: what was under it, or nothing.
    Select { ids: Vec<String> },
    /// The pointer came onto an object or left it.
    Hover { id: Option<String> },
    /// A drag of an object began.
    Grab { id: String },
    /// Where the object in hand is now, once per frame.
    Drag { id: String, position: [f64; 3] },
    /// An object was dropped after a drag.
    Move { id: String, position: [f64; 3] },
    /// The drag is over.
    Release { id: String },
    /// Pan or zoom came to rest.
    SetView { view: View },
}

#[derive(Serialize)]
pub enum Event {
    State {
        scene: Scene,
        selection: Selection,
        owners: Owners,
        view: Option<View>,
    },
    Scene {
        scene: Scene,
    },
    Selection {
        selection: Selection,
    },
    Owners {
        owners: Owners,
    },
    /// The holder of `id` has it at `position`; `ts` is the bus's on host `origin`, for
    /// `interpolate`.
    Drag {
        topic: String,
        id: String,
        position: [f64; 3],
        ts: u64,
        origin: HostId,
    },
    /// This pane's grab of `id` lost to another pane's.
    Denied {
        id: String,
        reason: String,
    },
    Hover {
        hover: Hover,
    },
    Focus {
        id: String,
    },
    View {
        view: View,
    },
    Rejected {
        reason: String,
    },
    Unsupported {
        version: u32,
    },
}

pub struct View2d {
    bus: Bus,
    rejected: String,
    scene: Scene,
    selection: Selection,
    owners: Owners,
    view: Option<View>,
}
impl Pane for View2d {
    const KIND: &'static str = "view2d";
    const MODULE: &'static str = include_str!("view2d.js");
    type Config = ();
    type Msg = Msg;
    type Event = Event;

    fn new(
        _config: (),
        ctx: Ctx,
    ) -> Self {
        let rejected = Topic::rejected(ctx.id);
        // Retained ones first.
        for topic in [
            Topic::SCENE,
            Topic::SELECTION,
            Topic::OWNERS,
            VIEW,
            Topic::PROTOCOL,
        ] {
            ctx.bus.subscribe(topic);
        }
        for topic in [Topic::HOVER, Topic::FOCUS, &rejected] {
            ctx.bus.subscribe(topic);
        }
        Self {
            bus: ctx.bus,
            rejected,
            scene: Scene::default(),
            selection: Selection::default(),
            owners: Owners::default(),
            view: None,
        }
    }
    fn update(
        &mut self,
        msg: Msg,
        _out: &mut Out<Event>,
    ) {
        match msg {
            Msg::Select { ids } => self.bus.command(Command::Select { ids }),
            Msg::Hover { id } => self
                .bus
                .publish(Topic::HOVER, serde_json::to_value(Hover { id }).unwrap()),
            Msg::Grab { id } => self.bus.command(Command::Grab { id }),
            Msg::Drag { id, position } => {
                let topic = Topic::drag(&id);
                let drag = Drag { id, position };
                self.bus
                    .publish(&topic, serde_json::to_value(drag).unwrap());
            }
            Msg::Move { id, position } => self.bus.command(Command::MoveObject { id, position }),
            Msg::Release { id } => self.bus.command(Command::Release { id }),
            Msg::SetView { view } => {
                if !view.is_valid() {
                    warn!("view2d: refusing view {view:?}");
                    return;
                }
                self.bus.retain(VIEW, serde_json::to_value(view).unwrap());
            }
        }
    }
    fn on_message(
        &mut self,
        topic: &str,
        msg: Message,
        out: &mut Out<Event>,
    ) {
        match topic {
            Topic::SCENE => {
                self.scene = Scene::from_message(&msg.value);
                // No wildcard topics: follow each object's drag stream by name.
                for object in &self.scene.objects {
                    self.bus.subscribe(&Topic::drag(&object.id));
                }
                out.emit(Event::Scene {
                    scene: self.scene.clone(),
                });
            }
            Topic::SELECTION => {
                self.selection = Selection::from_message(&msg.value);
                out.emit(Event::Selection {
                    selection: self.selection.clone(),
                });
            }
            Topic::OWNERS => {
                self.owners = Owners::from_message(&msg.value);
                out.emit(Event::Owners {
                    owners: self.owners.clone(),
                });
            }
            Topic::PROTOCOL => {
                let protocol = Protocol::from_message(&msg.value);
                if !protocol.supported() {
                    out.emit(Event::Unsupported {
                        version: protocol.version,
                    });
                }
            }
            Topic::HOVER => out.emit(Event::Hover {
                hover: Hover::from_message(&msg.value),
            }),
            Topic::FOCUS => {
                let Ok(focus) = serde_json::from_value::<Focus>(msg.value) else {
                    return;
                };
                out.emit(Event::Focus { id: focus.id });
            }
            VIEW => {
                let Ok(view) = serde_json::from_value::<View>(msg.value) else {
                    return;
                };
                if !view.is_valid() {
                    return;
                }
                self.view = Some(view);
                out.emit(Event::View { view });
            }
            _ if topic == self.rejected => {
                let reason = Rejected::reason(&msg.value);
                match serde_json::from_value::<Rejected>(msg.value) {
                    Ok(Rejected {
                        cmd: Command::Grab { id },
                        ..
                    }) => out.emit(Event::Denied { id, reason }),
                    _ => out.emit(Event::Rejected { reason }),
                }
            }
            _ => {
                // The host vouches for the holder of the topic's id, not of the message's.
                let Some(id) = Topic::drag_id(topic) else {
                    return;
                };
                let Ok(drag) = serde_json::from_value::<Drag>(msg.value) else {
                    return;
                };
                out.emit(Event::Drag {
                    topic: topic.to_owned(),
                    id: id.to_owned(),
                    position: drag.position,
                    ts: msg.ts.unwrap_or(0),
                    origin: msg.origin,
                });
            }
        }
    }
    fn snapshot(&self) -> Event {
        Event::State {
            scene: self.scene.clone(),
            selection: self.selection.clone(),
            owners: self.owners.clone(),
            view: self.view,
        }
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use pane_api::BusStore;
    use pane_api::PaneId;
    use serde_json::Value;
    use serde_json::json;

    use super::*;

    struct Nowhere;
    impl BusStore for Nowhere {
        fn subscribe(
            &self,
            _pane: PaneId,
            _topic: &str,
        ) {
            //
        }
        fn publish(
            &self,
            _pane: PaneId,
            _topic: &str,
            _value: Value,
            _retain: bool,
        ) {
            //
        }
    }

    fn hear(
        topic: &str,
        value: Value,
    ) -> Value {
        let id = PaneId::new(HostId(1), 1);
        let ctx = Ctx {
            id,
            storage: std::env::temp_dir(),
            bus: Bus::new(id, Arc::new(Nowhere)),
        };
        let mut pane = View2d::new((), ctx);
        let mut out = Out::new();
        let topic = topic.replace("<me>", &Topic::rejected(id));
        let msg = Message {
            value,
            origin: HostId(1),
            seq: None,
            ts: Some(42),
        };
        pane.on_message(&topic, msg, &mut out);
        serde_json::to_value(&out.events).unwrap()
    }

    #[test]
    fn test_drag_and_denial() {
        // The topic names the object, whatever the message says.
        let events = hear("drag/a", json!({ "id": "b", "position": [1, 2, 3] }));
        assert_eq!(
            events,
            json!([{ "Drag": { "topic": "drag/a", "id": "a", "position": [1.0, 2.0, 3.0],
                               "ts": 42, "origin": "00000000000000000000000000000001" } }])
        );
        assert_eq!(hear("drag/a", json!({ "id": "a" })), json!([]));
        let denied = json!({ "cmd": { "cmd": "grab", "id": "a" }, "reason": "held" });
        assert_eq!(
            hear("<me>", denied),
            json!([{ "Denied": { "id": "a", "reason": "held" } }])
        );
        let other = json!({ "cmd": { "cmd": "select", "ids": ["z"] }, "reason": "No z" });
        assert_eq!(
            hear("<me>", other),
            json!([{ "Rejected": { "reason": "No z" } }])
        );
    }

    #[test]
    fn test_view_shape() {
        let view: View = serde_json::from_value(json!({ "x": 1, "y": -2.5, "zoom": 40 })).unwrap();
        assert!(view.is_valid());
        assert!(serde_json::from_value::<View>(json!({ "x": 1, "y": 2 })).is_err());
        assert!(serde_json::from_value::<View>(json!({ "x": "1", "y": 2, "zoom": 3 })).is_err());
        for zoom in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(!View { zoom, ..view }.is_valid());
        }
        assert!(
            !View {
                x: f64::INFINITY,
                ..view
            }
            .is_valid()
        );
    }
}
