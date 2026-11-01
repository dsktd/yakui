use pane_api::StatelessPane;

#[derive(Default)]
pub struct Clock;
impl StatelessPane for Clock {
    const KIND: &'static str = "clock";
    const MODULE: &'static str = include_str!("clock.js");
}
