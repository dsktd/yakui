//! The `scene` in WebGL with an orbit camera: drag to orbit, wheel to zoom, click to select.

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

/// The camera at rest, shared by every view3d of the space and kept across reloads.
const CAMERA: &str = "view3d/camera";

/// The orbit camera: angles in radians, the distance to `target` in world units.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    yaw: f64,
    pitch: f64,
    distance: f64,
    target: [f64; 3],
}
impl Camera {
    fn is_valid(&self) -> bool {
        self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.distance.is_finite()
            && self.distance > 0.0
            && self.target.iter().all(|v| v.is_finite())
    }
}

#[derive(Serialize, Deserialize)]
pub enum Msg {
    /// A click: what was under it, or nothing.
    Select { ids: Vec<String> },
    /// The pointer came onto an object or left it.
    Hover { id: Option<String> },
    /// Orbit or zoom came to rest.
    SetCamera { camera: Camera },
}

#[derive(Serialize)]
pub enum Event {
    State {
        scene: Scene,
        selection: Selection,
        camera: Option<Camera>,
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
        camera: Camera,
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
    camera: Option<Camera>,
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
        // Retained ones first.
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
            camera: None,
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
            Msg::SetCamera { camera } => {
                if !camera.is_valid() {
                    warn!("view3d: refusing camera {camera:?}");
                    return;
                }
                self.bus
                    .retain(CAMERA, serde_json::to_value(camera).unwrap());
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
            CAMERA => {
                let Ok(camera) = serde_json::from_value::<Camera>(msg.value) else {
                    return;
                };
                if !camera.is_valid() {
                    return;
                }
                self.camera = Some(camera);
                out.emit(Event::Camera { camera });
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
            camera: self.camera,
        }
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_camera_shape() {
        let camera: Camera = serde_json::from_value(
            json!({ "yaw": 0.6, "pitch": 0.4, "distance": 12, "target": [0, 1, 0] }),
        )
        .unwrap();
        assert!(camera.is_valid());
        assert!(
            serde_json::from_value::<Camera>(json!({ "yaw": 0, "pitch": 0, "distance": 1 }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<Camera>(
                json!({ "yaw": 0, "pitch": 0, "distance": 1, "target": [0, 0] })
            )
            .is_err()
        );
        for distance in [0.0, -1.0, f64::NAN] {
            assert!(!Camera { distance, ..camera }.is_valid());
        }
        assert!(
            !Camera {
                target: [0.0, f64::INFINITY, 0.0],
                ..camera
            }
            .is_valid()
        );
    }
}
