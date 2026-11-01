//! A front view (x right, y up) of the `scene` on a canvas: pan, zoom, drag, click to select.

use pane_api::Bus;
use pane_api::Ctx;
use pane_api::Message;
use pane_api::Out;
use pane_api::Pane;
use pane_api::protocol::Command;
use pane_api::protocol::Focus;
use pane_api::protocol::Hover;
use pane_api::protocol::Object;
use pane_api::protocol::Scene;
use pane_api::protocol::Selection;
use pane_api::protocol::Topic;
use pane_api::protocol::VERSION;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

/// The pan and zoom at rest, shared by every view2d of the space and kept across reloads.
const VIEW: &str = "view2d/view";

#[derive(Serialize, Deserialize)]
pub enum Msg {
    /// A click: what was under it, or nothing.
    Select { ids: Vec<String> },
    /// The pointer came onto an object or left it.
    Hover { id: Option<String> },
    /// An object was dropped after a drag.
    Move { id: String, position: [f64; 3] },
    /// Pan or zoom came to rest.
    SetView { view: Value },
}

#[derive(Serialize)]
pub enum Event {
    State {
        scene: Scene,
        selection: Selection,
        view: Value,
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
    View {
        view: Value,
    },
    Rejected {
        reason: String,
    },
    Unsupported {
        version: Value,
    },
}

pub struct View2d {
    bus: Bus,
    rejected: String,
    scene: Scene,
    selection: Selection,
    view: Value,
    /// A retained `scene` arrived; without one by the time `protocol` comes, the space is
    /// fresh and this pane seeds it.
    seen_scene: bool,
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
        // Retained ones first, `protocol` after `scene`, so a missing scene is known by then.
        for topic in [Topic::SCENE, Topic::SELECTION, VIEW, Topic::PROTOCOL] {
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
            view: Value::Null,
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
            Msg::Move { id, position } => self.bus.command(Command::MoveObject { id, position }),
            Msg::SetView { view } => self.bus.retain(VIEW, view),
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
                if msg.value["version"] != json!(VERSION) {
                    out.emit(Event::Unsupported {
                        version: msg.value["version"].clone(),
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
            VIEW => {
                self.view = msg.value;
                out.emit(Event::View {
                    view: self.view.clone(),
                });
            }
            _ if topic == self.rejected => out.emit(Event::Rejected {
                reason: msg.value["reason"]
                    .as_str()
                    .unwrap_or("rejected")
                    .to_owned(),
            }),
            _ => {}
        }
    }
    fn snapshot(&self) -> Event {
        Event::State {
            scene: self.scene.clone(),
            selection: self.selection.clone(),
            view: self.view.clone(),
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
