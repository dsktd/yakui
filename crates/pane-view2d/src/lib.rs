//! A front view (x right, y up) of the `scene` on a canvas: pan, zoom, drag, click to select.

use pane_api::Bus;
use pane_api::Ctx;
use pane_api::Message;
use pane_api::Out;
use pane_api::Pane;
use pane_api::protocol::Command;
use pane_api::protocol::Focus;
use pane_api::protocol::Hover;
use pane_api::protocol::Protocol;
use pane_api::protocol::Rejected;
use pane_api::protocol::Scene;
use pane_api::protocol::Selection;
use pane_api::protocol::Topic;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

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
        version: u32,
    },
}

pub struct View2d {
    bus: Bus,
    rejected: String,
    scene: Scene,
    selection: Selection,
    view: Value,
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
                reason: Rejected::reason(&msg.value),
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
