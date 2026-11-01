//! A front view (x right, y up) of the `scene` on a canvas: pan, zoom, drag, click to select.

use log::warn;
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
    /// An object was dropped after a drag.
    Move { id: String, position: [f64; 3] },
    /// Pan or zoom came to rest.
    SetView { view: View },
}

#[derive(Serialize)]
pub enum Event {
    State {
        scene: Scene,
        selection: Selection,
        view: Option<View>,
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
            Msg::Move { id, position } => self.bus.command(Command::MoveObject { id, position }),
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
                let Ok(view) = serde_json::from_value::<View>(msg.value) else {
                    return;
                };
                if !view.is_valid() {
                    return;
                }
                self.view = Some(view);
                out.emit(Event::View { view });
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
            view: self.view,
        }
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use super::*;

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
