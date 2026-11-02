//! The shared vocabulary of PROTOCOL.md: topics, their message shapes, and the commands the
//! host applies. Every pane uses these; none keeps its own copy.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

use crate::PaneId;

pub const VERSION: u32 = 1;

/// The standard topic names. `RETAINED` ones are written by the host alone.
pub struct Topic;
impl Topic {
    pub const PROTOCOL: &'static str = "protocol";
    pub const SCENE: &'static str = "scene";
    pub const SELECTION: &'static str = "selection";
    pub const OWNERS: &'static str = "owners";
    pub const HOVER: &'static str = "hover";
    pub const TOOL: &'static str = "tool";
    pub const FOCUS: &'static str = "focus";
    pub const CMD: &'static str = "cmd";
    pub const RETAINED: [&'static str; 4] =
        [Self::PROTOCOL, Self::SCENE, Self::SELECTION, Self::OWNERS];
    pub const TRANSIENT: [&'static str; 4] = [Self::HOVER, Self::TOOL, Self::FOCUS, Self::CMD];

    /// Where the host answers `pane`'s invalid commands.
    pub fn rejected(pane: PaneId) -> String {
        format!("rejected/{pane}")
    }
    /// Where the pane holding object `id` streams it while dragging.
    pub fn drag(id: &str) -> String {
        format!("drag/{id}")
    }
    /// The object a `drag/<id>` topic is about.
    pub fn drag_id(topic: &str) -> Option<&str> {
        topic.strip_prefix("drag/").filter(|id| !id.is_empty())
    }
    pub fn is_standard(topic: &str) -> bool {
        Self::RETAINED.contains(&topic)
            || Self::TRANSIENT.contains(&topic)
            || topic.starts_with("rejected/")
            || topic.starts_with("drag/")
    }
}

/// One thing in the `scene`. Fields a writer does not know stay in `extra`, untouched.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Object {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub position: [f64; 3],
    #[serde(default = "unit")]
    pub size: [f64; 3],
    #[serde(default = "grey")]
    pub color: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub props: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Object {
    /// The same as a `{}` message: a grey unit box without an id.
    fn default() -> Self {
        serde_json::from_value(Value::Object(Map::new())).unwrap()
    }
}

fn unit() -> [f64; 3] {
    [1.0, 1.0, 1.0]
}

fn grey() -> String {
    "#888888".to_owned()
}

/// The `protocol` message. A malformed one reads as version 0, which no pane supports.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Protocol {
    pub version: u32,
}
impl Protocol {
    pub fn from_message(msg: &Value) -> Self {
        serde_json::from_value(msg.clone()).unwrap_or_default()
    }
    pub fn supported(&self) -> bool {
        self.version == VERSION
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Scene {
    pub objects: Vec<Object>,
}
impl Scene {
    /// The message as a scene, keeping the objects that parse; a malformed one reads as
    /// empty.
    pub fn from_message(msg: &Value) -> Self {
        let mut scene = Scene::default();
        let Some(objects) = msg.get("objects").and_then(Value::as_array) else {
            return scene;
        };
        for object in objects {
            let Ok(object) = Object::deserialize(object) else {
                continue;
            };
            if object.id.is_empty() {
                continue;
            }
            scene.objects.push(object);
        }
        scene
    }
    pub fn object(
        &self,
        id: &str,
    ) -> Option<&Object> {
        self.objects.iter().find(|object| object.id == id)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Selection {
    pub ids: Vec<String>,
}
impl Selection {
    pub fn from_message(msg: &Value) -> Self {
        serde_json::from_value(msg.clone()).unwrap_or_default()
    }
}

/// The `owners` message: which pane holds which object, by `grab` until `release`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Owners {
    pub owners: BTreeMap<String, PaneId>,
}
impl Owners {
    pub fn from_message(msg: &Value) -> Self {
        serde_json::from_value(msg.clone()).unwrap_or_default()
    }
    pub fn holder(
        &self,
        id: &str,
    ) -> Option<PaneId> {
        self.owners.get(id).copied()
    }
}

/// A `drag/<id>` message: where the holder has object `id` now, between `grab` and the
/// `move_object` that drops it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Drag {
    pub id: String,
    pub position: [f64; 3],
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Hover {
    pub id: Option<String>,
}
impl Hover {
    pub fn from_message(msg: &Value) -> Self {
        serde_json::from_value(msg.clone()).unwrap_or_default()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Focus {
    pub id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Tool {
    pub name: String,
}

/// A coarse write to `scene` or `selection`: the result of an interaction, never a stream.
/// Published on `cmd`; the host validates and applies it whole, or rejects it whole.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Select {
        ids: Vec<String>,
    },
    /// `object.id` empty: the host assigns one.
    AddObject {
        object: Object,
    },
    RemoveObjects {
        ids: Vec<String>,
    },
    /// On every object of `ids` or on none: a known key of [`Object`] (not `id`), or any
    /// other key into `props`.
    SetProperty {
        ids: Vec<String>,
        key: String,
        value: Value,
    },
    MoveObject {
        id: String,
        position: [f64; 3],
    },
    /// Hold `id` for a drag: only the holder may stream `drag/<id>` or move it.
    Grab {
        id: String,
    },
    Release {
        id: String,
    },
}

/// A command the host dropped, published on `rejected/<pane>` to the pane that sent it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rejected {
    pub cmd: Command,
    pub reason: String,
}
impl Rejected {
    /// The reason alone, whatever else the message holds.
    pub fn reason(msg: &Value) -> String {
        msg["reason"].as_str().unwrap_or("rejected").to_owned()
    }
}

#[cfg(test)]
mod test {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_scene_from_message() {
        assert!(Scene::from_message(&json!("banana")).objects.is_empty());
        assert!(
            Scene::from_message(&json!({ "objects": 3 }))
                .objects
                .is_empty()
        );
        let scene = Scene::from_message(&json!({ "objects": [
            { "id": "a", "name": "A", "kind": "box", "position": [1, 2, 3], "size": [1, 1, 1],
              "color": "#ff0000", "tag": "kept" },
            { "name": "no id" },
            { "id": "b", "position": "not a vector" },
            7,
        ] }));
        assert_eq!(scene.objects.len(), 1);
        assert_eq!(scene.object("a").unwrap().extra["tag"], json!("kept"));
        assert!(scene.object("b").is_none());
    }

    #[test]
    fn test_object_roundtrip_keeps_extra() {
        let value = json!({ "id": "a", "name": "A", "kind": "box", "position": [0.0, 0.0, 0.0],
                            "size": [1.0, 1.0, 1.0], "color": "#ffffff", "tag": "kept" });
        let object: Object = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(&object).unwrap(), value);
    }

    #[test]
    fn test_command_tag() {
        let cmd = Command::Select {
            ids: vec!["a".to_owned()],
        };
        assert_eq!(
            serde_json::to_value(&cmd).unwrap(),
            json!({ "cmd": "select", "ids": ["a"] })
        );
        let cmd: Command = serde_json::from_value(
            json!({ "cmd": "move_object", "id": "a", "position": [1, 2, 3] }),
        )
        .unwrap();
        assert!(matches!(cmd, Command::MoveObject { .. }));
    }

    #[test]
    fn test_topics() {
        let pane = PaneId::new(crate::HostId(1), 7);
        assert_eq!(
            Topic::rejected(pane),
            "rejected/00000000000000000000000000000001-7"
        );
        assert!(Topic::is_standard(&Topic::rejected(pane)));
        assert!(Topic::is_standard("scene"));
        assert!(!Topic::is_standard("view3d/camera"));
        assert!(Topic::is_standard(&Topic::drag("a")));
        assert_eq!(Topic::drag_id("drag/a"), Some("a"));
        assert_eq!(Topic::drag_id("drag/"), None);
        assert_eq!(Topic::drag_id("view2d/a"), None);
        let owners = Owners::from_message(&json!({ "owners": { "a": pane } }));
        assert_eq!(owners.holder("a"), Some(pane));
        assert_eq!(owners.holder("b"), None);
        assert!(
            Owners::from_message(&json!({ "owners": { "a": 3 } }))
                .owners
                .is_empty()
        );
        assert!(Selection::from_message(&json!(["a"])).ids.is_empty());
        assert_eq!(Hover::from_message(&json!({ "id": null })).id, None);
        assert!(Protocol::from_message(&json!({ "version": 1 })).supported());
        assert!(!Protocol::from_message(&json!({ "version": 2 })).supported());
        assert_eq!(Protocol::from_message(&json!("banana")).version, 0);
        assert_eq!(
            Rejected::reason(&json!({ "reason": "No object a" })),
            "No object a"
        );
        assert_eq!(Rejected::reason(&json!(3)), "rejected");
    }
}
