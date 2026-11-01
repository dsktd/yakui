//! Panes by type name, with their types erased to JSON at the boundary.

use std::collections::BTreeMap;

use pane_api::Ctx;
use pane_api::Out;
use pane_api::Pane;
use serde_json::Value;

use crate::Result;

pub trait AnyPane: Send {
    fn update(
        &mut self,
        msg: Value,
    ) -> Result<Vec<Value>>;
    fn snapshot(&self) -> Value;
}
impl<P: Pane> AnyPane for P {
    fn update(
        &mut self,
        msg: Value,
    ) -> Result<Vec<Value>> {
        let msg = serde_json::from_value(msg)?;
        let mut out = Out::new();
        P::update(self, msg, &mut out);
        let mut events = Vec::with_capacity(out.events.len());
        for event in out.events {
            events.push(serde_json::to_value(event)?);
        }
        Ok(events)
    }
    fn snapshot(&self) -> Value {
        serde_json::to_value(P::snapshot(self)).unwrap()
    }
}

type Make = fn(Value, Ctx) -> Result<Box<dyn AnyPane>>;

fn make<P: Pane>(
    config: Value,
    ctx: Ctx,
) -> Result<Box<dyn AnyPane>> {
    let config = match config {
        Value::Null => P::Config::default(),
        config => serde_json::from_value(config)?,
    };
    Ok(Box::new(P::new(config, ctx)))
}

struct Kind {
    make: Make,
    module: &'static str,
}

#[derive(Default)]
pub struct Registry(BTreeMap<&'static str, Kind>);
impl Registry {
    pub fn add<P: Pane>(&mut self) {
        self.0.insert(
            P::KIND,
            Kind {
                make: make::<P>,
                module: P::MODULE,
            },
        );
    }
    pub fn kinds(&self) -> Vec<&'static str> {
        self.0.keys().copied().collect()
    }
    pub fn module(
        &self,
        kind: &str,
    ) -> Option<&'static str> {
        self.0.get(kind).map(|k| k.module)
    }
    pub fn make(
        &self,
        kind: &str,
        config: Value,
        ctx: Ctx,
    ) -> Result<Box<dyn AnyPane>> {
        let Some(k) = self.0.get(kind) else {
            return Err(anyhow::anyhow!("Unknown pane kind {kind}"));
        };
        std::fs::create_dir_all(&ctx.storage)?;
        (k.make)(config, ctx)
    }
}
