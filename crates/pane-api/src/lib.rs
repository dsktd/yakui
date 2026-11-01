//! The one trait a pane implements. Panes depend on this crate and nothing else of yakui.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub mod protocol;

use protocol::Command;
use protocol::Topic;

/// The host a pane or message comes from: 128 random bits, made once per host. Zero stands
/// for a layout written before hosts had ids.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct HostId(pub u128);
impl std::fmt::Display for HostId {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}
impl FromStr for HostId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        if s.len() != 32 {
            return Err(format!("Host id {s:?} is not 32 hex digits"));
        }
        u128::from_str_radix(s, 16)
            .map(Self)
            .or(Err(format!("Host id {s:?} is not hex")))
    }
}
impl TryFrom<String> for HostId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}
impl From<HostId> for String {
    fn from(id: HostId) -> String {
        id.to_string()
    }
}

/// `<host>-<local>` on the wire, in layout files and in URLs; a bare number is a pane from
/// before hosts had ids.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct PaneId {
    pub host: HostId,
    pub local: u32,
}
impl PaneId {
    pub const fn new(
        host: HostId,
        local: u32,
    ) -> Self {
        Self { host, local }
    }
}
impl std::fmt::Display for PaneId {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(f, "{}-{}", self.host, self.local)
    }
}
impl FromStr for PaneId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let (host, local) = match s.split_once('-') {
            Some((host, local)) => (host.parse()?, local),
            None => (HostId(0), s),
        };
        let local = local
            .parse()
            .or(Err(format!("Pane id {s:?} does not end in a number")))?;
        Ok(Self { host, local })
    }
}
impl TryFrom<String> for PaneId {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        s.parse()
    }
}
impl From<PaneId> for String {
    fn from(id: PaneId) -> String {
        id.to_string()
    }
}

/// What a pane gets from the host: its id, a directory that is its own, and the space's bus.
pub struct Ctx {
    pub id: PaneId,
    pub storage: PathBuf,
    pub bus: Bus,
}

/// One message as the bus delivers it. A retained message also carries `seq`, which counts
/// up per topic at its origin, so two hosts' copies of a topic can be ordered later.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message {
    pub value: Value,
    pub origin: HostId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
}

/// The host's side of the bus, one per space. The host implements it; panes only see it
/// through [`Bus`].
pub trait BusStore: Send + Sync {
    fn subscribe(
        &self,
        pane: PaneId,
        topic: &str,
    );
    fn publish(
        &self,
        pane: PaneId,
        topic: &str,
        value: Value,
        retain: bool,
    );
}

/// A pane's end of the space's message bus (PROTOCOL.md). Subscribe by topic in `new()`;
/// messages come back through [`Pane::on_message`] once the update that published them has
/// returned, a retained topic's last message first. Standard retained topics (`scene`,
/// `selection`) change only through [`Bus::command`].
#[derive(Clone)]
pub struct Bus {
    pane: PaneId,
    store: Arc<dyn BusStore>,
}
impl Bus {
    pub fn new(
        pane: PaneId,
        store: Arc<dyn BusStore>,
    ) -> Self {
        Self { pane, store }
    }
    pub fn subscribe(
        &self,
        topic: &str,
    ) {
        self.store.subscribe(self.pane, topic);
    }
    /// A transient message: current subscribers get it, nobody else ever.
    pub fn publish(
        &self,
        topic: &str,
        value: Value,
    ) {
        self.store.publish(self.pane, topic, value, false);
    }
    /// A retained message on a private topic (`<pane kind>/<name>`): kept, persisted, and
    /// delivered to every later subscriber.
    pub fn retain(
        &self,
        topic: &str,
        value: Value,
    ) {
        self.store.publish(self.pane, topic, value, true);
    }
    /// Publish on `cmd`. The host applies it or answers on `rejected/<pane>`.
    pub fn command(
        &self,
        cmd: Command,
    ) {
        self.publish(Topic::CMD, serde_json::to_value(cmd).unwrap());
    }
}

/// Events a pane emits during one [`Pane::update`].
pub struct Out<E> {
    pub events: Vec<E>,
}
impl<E> Out<E> {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }
    pub fn emit(
        &mut self,
        event: E,
    ) {
        self.events.push(event);
    }
}
impl<E> Default for Out<E> {
    fn default() -> Self {
        Self::new()
    }
}

pub trait Pane: Send + 'static {
    /// Stable type name, used by the registry and layout files.
    const KIND: &'static str;
    /// The web half: one self-contained ES module exporting `mount(root, {send, onEvent})`.
    const MODULE: &'static str;
    type Config: Serialize + DeserializeOwned + Default;
    /// Client -> pane, coarse.
    type Msg: Serialize + DeserializeOwned;
    /// Pane -> client, deltas; `snapshot` returns the whole state as one.
    type Event: Serialize;

    fn new(
        config: Self::Config,
        ctx: Ctx,
    ) -> Self;
    fn update(
        &mut self,
        msg: Self::Msg,
        out: &mut Out<Self::Event>,
    );
    /// A message on a topic this pane subscribed to. Default: nothing.
    fn on_message(
        &mut self,
        _topic: &str,
        _msg: Message,
        _out: &mut Out<Self::Event>,
    ) {
        //
    }
    fn snapshot(&self) -> Self::Event;
}

/// A pane with no Rust state: only a kind and a web half.
pub trait StatelessPane: Default + Send + 'static {
    const KIND: &'static str;
    const MODULE: &'static str;
}
impl<P: StatelessPane> Pane for P {
    const KIND: &'static str = P::KIND;
    const MODULE: &'static str = P::MODULE;
    type Config = ();
    type Msg = ();
    type Event = ();

    fn new(
        _config: (),
        _ctx: Ctx,
    ) -> Self {
        P::default()
    }
    fn update(
        &mut self,
        _msg: (),
        _out: &mut Out<()>,
    ) {
        //
    }
    fn snapshot(&self) {
        //
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_pane_id() {
        let host: HostId = "0123456789abcdef0123456789abcdef".parse().unwrap();
        let id = PaneId::new(host, 7);
        assert_eq!(id.to_string(), "0123456789abcdef0123456789abcdef-7");
        assert_eq!(id.to_string().parse::<PaneId>().unwrap(), id);
        assert_eq!("7".parse::<PaneId>().unwrap(), PaneId::new(HostId(0), 7));
        assert!("x-7".parse::<PaneId>().is_err());
        assert!(
            "0123456789abcdef0123456789abcdef-x"
                .parse::<PaneId>()
                .is_err()
        );
        assert!("".parse::<PaneId>().is_err());
        assert_eq!(
            serde_json::to_value(id).unwrap(),
            serde_json::Value::String(id.to_string())
        );
        let map: std::collections::BTreeMap<PaneId, u8> =
            serde_json::from_str(r#"{"0123456789abcdef0123456789abcdef-7": 1, "2": 2}"#).unwrap();
        assert_eq!(map[&id], 1);
        assert_eq!(map[&PaneId::new(HostId(0), 2)], 2);
    }
}
