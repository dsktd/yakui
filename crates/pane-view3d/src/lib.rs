//! The `scene` in WebGL with an orbit camera: drag to orbit, wheel to zoom, click to select.

use pane_api::Bus;
use pane_api::Ctx;
use pane_api::Message;
use pane_api::Out;
use pane_api::Pane;
use pane_api::protocol::Command;
use pane_api::protocol::Focus;
use pane_api::protocol::Hover;
use pane_api::protocol::Object;
use pane_api::protocol::Protocol;
use pane_api::protocol::Rejected;
use pane_api::protocol::Scene;
use pane_api::protocol::Selection;
use pane_api::protocol::Topic;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// The camera at rest, shared by every view3d of the space and kept across reloads.
const CAMERA: &str = "view3d/camera";

#[derive(Serialize, Deserialize)]
pub enum Msg {
    /// A click: what was under it, or nothing.
    Select { ids: Vec<String> },
    /// The pointer came onto an object or left it.
    Hover { id: Option<String> },
    /// Orbit or zoom came to rest.
    SetCamera { camera: Value },
}

#[derive(Serialize)]
pub enum Event {
    State {
        scene: Scene,
        selection: Selection,
        camera: Value,
    },
    Scene {
        scene: Scene,
    },
    Selection {
        selection: Selection,
    },
    Hover {
        hover: Hover,
    },
    Focus {
        id: String,
    },
    Camera {
        camera: Value,
    },
    Rejected {
        reason: String,
    },
    Unsupported {
        version: u32,
    },
}

pub struct View3d {
    bus: Bus,
    rejected: String,
    scene: Scene,
    selection: Selection,
    camera: Value,
    /// A retained `scene` arrived; without one by the time `protocol` comes, the space is
    /// fresh and this pane seeds it.
    seen_scene: bool,
}
impl Pane for View3d {
    const KIND: &'static str = "view3d";
    const MODULE: &'static str = include_str!("view3d.js");
    type Config = ();
    type Msg = Msg;
    type Event = Event;

    fn new(
        _config: (),
        ctx: Ctx,
    ) -> Self {
        let rejected = Topic::rejected(ctx.id);
        // Retained ones first, `protocol` after `scene`, so a missing scene is known by then.
        for topic in [Topic::SCENE, Topic::SELECTION, CAMERA, Topic::PROTOCOL] {
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
            camera: Value::Null,
            seen_scene: false,
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
            Msg::SetCamera { camera } => self.bus.retain(CAMERA, camera),
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
                self.seen_scene = true;
                self.scene = Scene::from_message(&msg.value);
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
            Topic::PROTOCOL => {
                let protocol = Protocol::from_message(&msg.value);
                if !protocol.supported() {
                    out.emit(Event::Unsupported {
                        version: protocol.version,
                    });
                    return;
                }
                if !self.seen_scene {
                    seed(&self.bus);
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
            CAMERA => {
                self.camera = msg.value;
                out.emit(Event::Camera {
                    camera: self.camera.clone(),
                });
            }
            _ if topic == self.rejected => out.emit(Event::Rejected {
                reason: Rejected::reason(&msg.value),
            }),
            _ => {}
        }
    }
    fn snapshot(&self) -> Event {
        Event::State {
            scene: self.scene.clone(),
            selection: self.selection.clone(),
            camera: self.camera.clone(),
        }
    }
}

/// Four objects so a fresh space shows something. Fixed ids: a second viewport seeding at
/// the same moment is refused object by object and the scene stays at four.
fn seed(bus: &Bus) {
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
    for (id, name, kind, position, size, color) in objects {
        let object = Object {
            id: id.to_owned(),
            name: name.to_owned(),
            kind: kind.to_owned(),
            position,
            size,
            color: color.to_owned(),
            ..Default::default()
        };
        bus.command(Command::AddObject { object });
    }
}
