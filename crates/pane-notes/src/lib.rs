use std::path::PathBuf;

use log::warn;
use pane_api::Ctx;
use pane_api::Out;
use pane_api::Pane;
use serde::Deserialize;
use serde::Serialize;

#[derive(Serialize, Deserialize)]
pub enum Msg {
    SetText { text: String },
}

#[derive(Serialize)]
pub enum Event {
    Text { text: String },
}

/// One text file in the pane's storage dir.
pub struct Notes {
    text: String,
    path: PathBuf,
}
impl Pane for Notes {
    const KIND: &'static str = "notes";
    const MODULE: &'static str = include_str!("notes.js");
    type Config = ();
    type Msg = Msg;
    type Event = Event;

    fn new(
        _config: (),
        ctx: Ctx,
    ) -> Self {
        let path = ctx.storage.join("notes.txt");
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        Self { text, path }
    }
    fn update(
        &mut self,
        msg: Msg,
        out: &mut Out<Event>,
    ) {
        let Msg::SetText { text } = msg;
        if let Err(err) = self.persist(&text) {
            warn!("notes {}: {err}", self.path.display());
        }
        self.text = text;
        out.emit(self.snapshot());
    }
    fn snapshot(&self) -> Event {
        Event::Text {
            text: self.text.clone(),
        }
    }
}
impl Notes {
    /// Write next to the file and rename over it, so a kill never leaves it half written.
    fn persist(
        &self,
        text: &str,
    ) -> std::io::Result<()> {
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &self.path)
    }
}
