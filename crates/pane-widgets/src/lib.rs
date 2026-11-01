//! The widget gallery: every widget alone with sample props and a log of what it emits.

use pane_api::StatelessPane;

#[derive(Default)]
pub struct Widgets;
impl StatelessPane for Widgets {
    const KIND: &'static str = "widgets";
    const MODULE: &'static str = include_str!("widgets.js");
}
