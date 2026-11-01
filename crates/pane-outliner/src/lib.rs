//! The `scene` as a tree, by kind; selection and hover both ways, double click to focus.

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

#[derive(Serialize, Deserialize)]
pub enum Msg {
    Select {
        ids: Vec<String>,
    },
    Hover {
        id: Option<String>,
    },
    /// Double click: ask the viewports to look at this object.
    Focus {
        id: String,
    },
}

#[derive(Serialize)]
pub enum Event {
    State { scene: Scene, selection: Selection },
    Hover { hover: Hover },
    Rejected { reason: String },
    Unsupported { version: u32 },
}

pub struct Outliner {
    bus: Bus,
    rejected: String,
    scene: Scene,
    selection: Selection,
}
impl Pane for Outliner {
    const KIND: &'static str = "outliner";
    const MODULE: &'static str = include_str!("outliner.js");
    type Config = ();
    type Msg = Msg;
    type Event = Event;

    fn new(
        _config: (),
        ctx: Ctx,
    ) -> Self {
        let rejected = Topic::rejected(ctx.id);
        for topic in [
            Topic::SCENE,
            Topic::SELECTION,
            Topic::PROTOCOL,
            Topic::HOVER,
            &rejected,
        ] {
            ctx.bus.subscribe(topic);
        }
        Self {
            bus: ctx.bus,
            rejected,
            scene: Scene::default(),
            selection: Selection::default(),
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
            Msg::Focus { id } => self
                .bus
                .publish(Topic::FOCUS, serde_json::to_value(Focus { id }).unwrap()),
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
                out.emit(self.snapshot());
            }
            Topic::SELECTION => {
                self.selection = Selection::from_message(&msg.value);
                out.emit(self.snapshot());
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
        }
    }
}
