//! A grid of colours; picking one colours every selected object.

use pane_api::Bus;
use pane_api::Ctx;
use pane_api::Message;
use pane_api::Out;
use pane_api::Pane;
use pane_api::protocol::Command;
use pane_api::protocol::Protocol;
use pane_api::protocol::Rejected;
use pane_api::protocol::Scene;
use pane_api::protocol::Selection;
use pane_api::protocol::Topic;
use serde::Deserialize;
use serde::Serialize;
use serde_json::json;

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub colors: Vec<String>,
}
impl Default for Config {
    fn default() -> Self {
        let colors = [
            "#e06c3a", "#e0b03a", "#a8d03a", "#3ad06c", "#3ac0c0", "#3a8ee0", "#6c3ae0", "#c03ae0",
            "#e03a8e", "#ffffff", "#8a8a8a", "#222222",
        ];
        Self {
            colors: colors.iter().map(|c| c.to_string()).collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum Msg {
    Pick { color: String },
}

#[derive(Serialize)]
pub enum Event {
    /// The grid, the selected ids and the first selected object's colour, if any.
    State {
        colors: Vec<String>,
        selected: Vec<String>,
        color: Option<String>,
    },
    Rejected {
        reason: String,
    },
    Unsupported {
        version: u32,
    },
}

pub struct Palette {
    bus: Bus,
    rejected: String,
    colors: Vec<String>,
    scene: Scene,
    selection: Selection,
}
impl Pane for Palette {
    const KIND: &'static str = "palette";
    const MODULE: &'static str = include_str!("palette.js");
    type Config = Config;
    type Msg = Msg;
    type Event = Event;

    fn new(
        config: Config,
        ctx: Ctx,
    ) -> Self {
        let rejected = Topic::rejected(ctx.id);
        for topic in [Topic::SCENE, Topic::SELECTION, Topic::PROTOCOL, &rejected] {
            ctx.bus.subscribe(topic);
        }
        Self {
            bus: ctx.bus,
            rejected,
            colors: config.colors,
            scene: Scene::default(),
            selection: Selection::default(),
        }
    }
    fn update(
        &mut self,
        msg: Msg,
        out: &mut Out<Event>,
    ) {
        let Msg::Pick { color } = msg;
        if self.selection.ids.is_empty() {
            out.emit(Event::Rejected {
                reason: "Nothing is selected".to_owned(),
            });
            return;
        }
        self.bus.command(Command::SetProperty {
            ids: self.selection.ids.clone(),
            key: "color".to_owned(),
            value: json!(color),
        });
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
            _ if topic == self.rejected => out.emit(Event::Rejected {
                reason: Rejected::reason(&msg.value),
            }),
            _ => {}
        }
    }
    fn snapshot(&self) -> Event {
        let color = self
            .selection
            .ids
            .first()
            .and_then(|id| self.scene.object(id))
            .map(|object| object.color.clone());
        Event::State {
            colors: self.colors.clone(),
            selected: self.selection.ids.clone(),
            color,
        }
    }
}
