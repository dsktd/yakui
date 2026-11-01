//! The selected object's properties; an edit goes back into the `scene` as one command.

use pane_api::Bus;
use pane_api::Ctx;
use pane_api::Message;
use pane_api::Out;
use pane_api::Pane;
use pane_api::protocol::Command;
use pane_api::protocol::Object;
use pane_api::protocol::Scene;
use pane_api::protocol::Selection;
use pane_api::protocol::Topic;
use pane_api::protocol::VERSION;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

#[derive(Serialize, Deserialize)]
pub enum Msg {
    /// A field was committed: a known key of the object, or any other into its `props`.
    Set {
        id: String,
        key: String,
        value: Value,
    },
}

#[derive(Serialize)]
pub enum Event {
    /// The first selected object, or none.
    Object {
        object: Option<Object>,
    },
    Rejected {
        reason: String,
    },
    Unsupported {
        version: Value,
    },
}

pub struct Inspector {
    bus: Bus,
    rejected: String,
    scene: Scene,
    selection: Selection,
}
impl Pane for Inspector {
    const KIND: &'static str = "inspector";
    const MODULE: &'static str = include_str!("inspector.js");
    type Config = ();
    type Msg = Msg;
    type Event = Event;

    fn new(
        _config: (),
        ctx: Ctx,
    ) -> Self {
        let rejected = Topic::rejected(ctx.id);
        for topic in [Topic::SCENE, Topic::SELECTION, Topic::PROTOCOL, &rejected] {
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
        let Msg::Set { id, key, value } = msg;
        // A position edit is a move; anything else is a property. Both are checked by the host.
        let cmd = match (
            key.as_str(),
            serde_json::from_value::<[f64; 3]>(value.clone()),
        ) {
            ("position", Ok(position)) => Command::MoveObject { id, position },
            _ => Command::SetProperty { id, key, value },
        };
        self.bus.command(cmd);
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
            Topic::PROTOCOL if msg.value["version"] != json!(VERSION) => {
                out.emit(Event::Unsupported {
                    version: msg.value["version"].clone(),
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
        let object = self
            .selection
            .ids
            .first()
            .and_then(|id| self.scene.object(id))
            .cloned();
        Event::Object { object }
    }
}
