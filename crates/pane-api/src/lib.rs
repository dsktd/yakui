//! The one trait a pane implements. Panes depend on this crate and nothing else of yakui.

use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(
    Serialize, serde::Deserialize, Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord,
)]
pub struct PaneId(pub u32);
impl std::fmt::Display for PaneId {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What a pane gets from the host: its id and a directory that is its own.
pub struct Ctx {
    pub id: PaneId,
    pub storage: PathBuf,
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
